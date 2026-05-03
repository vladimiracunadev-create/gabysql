use crate::bptree::init_leaf_page;
use crate::catalog::{validate_create_table, Catalog, Column, ColumnType, TableMeta};
use crate::storage::Pager;
use crate::{DbError, DbResult};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    CreateTable(CreateTableStmt),
    Insert(InsertStmt),
    Select(SelectStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTableStmt {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub primary_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub type_name: String,
    pub primary_key: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub table: String,
    pub columns: Vec<String>,
    pub values: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectStmt {
    pub table: String,
    pub columns: Vec<String>,
    pub where_clause: Option<WhereClause>,
    pub limit: Option<usize>,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WhereClause {
    Eq { column: String, value: i64 },
    Between { column: String, from: i64, to: i64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Float(f64),
    Bool(bool),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub message: Option<String>,
}

pub struct Engine<'a> {
    pager: &'a mut Pager,
}

impl<'a> Engine<'a> {
    pub fn new(pager: &'a mut Pager) -> Self {
        Self { pager }
    }

    pub fn exec(&mut self, statement: Statement) -> DbResult<ResultSet> {
        match statement {
            Statement::CreateTable(stmt) => self.exec_create(stmt),
            Statement::Insert(stmt) => self.exec_insert(stmt),
            Statement::Select(stmt) => self.exec_select(stmt),
        }
    }

    fn exec_create(&mut self, stmt: CreateTableStmt) -> DbResult<ResultSet> {
        let mut columns = Vec::with_capacity(stmt.columns.len());
        let mut primary_key = stmt.primary_key.clone();
        for column in stmt.columns {
            let column_type = ColumnType::from_sql(&column.type_name)?;
            if column.primary_key {
                primary_key = column.name.clone();
            }
            columns.push(Column {
                name: column.name,
                column_type,
            });
        }

        let mut meta = TableMeta {
            name: stmt.name,
            primary_key,
            columns,
            root_page: 0,
        };
        validate_create_table(&meta)?;

        {
            let mut catalog = Catalog::open(self.pager);
            if catalog.get_table(&meta.name)?.is_some() {
                return Err(DbError::new(format!("tabla {} ya existe", meta.name)));
            }
        }

        let root_page = self.pager.new_page()?;
        let mut leaf_page = vec![0; self.pager.page_size()];
        init_leaf_page(&mut leaf_page);
        self.pager.write_page(root_page, &leaf_page, true)?;
        meta.root_page = root_page;

        let mut catalog = Catalog::open(self.pager);
        catalog.put_table(&meta)?;

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    fn exec_insert(&mut self, stmt: InsertStmt) -> DbResult<ResultSet> {
        if stmt.columns.len() != stmt.values.len() {
            return Err(DbError::new("cantidad columnas != valores"));
        }
        let mut catalog = Catalog::open(self.pager);
        let meta = catalog
            .get_table(&stmt.table)?
            .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?;

        let mut seen = HashSet::new();
        let mut values = HashMap::new();
        for (column_name, value) in stmt.columns.into_iter().zip(stmt.values) {
            let normalized = normalize_ident(&column_name);
            if !seen.insert(normalized.clone()) {
                return Err(DbError::new("columna duplicada en INSERT"));
            }
            if meta.column(&normalized).is_none() {
                return Err(DbError::new(format!("columna no existe: {}", column_name)));
            }
            values.insert(normalized, value);
        }

        let (pk, row_bytes) = encode_row(&meta, &values)?;
        catalog.insert_row(meta.root_page, pk, row_bytes)?;
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    fn exec_select(&mut self, stmt: SelectStmt) -> DbResult<ResultSet> {
        let mut catalog = Catalog::open(self.pager);
        let meta = catalog
            .get_table(&stmt.table)?
            .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?;

        let selected_columns = resolve_selected_columns(&meta, &stmt.columns)?;
        let output_columns: Vec<String> = selected_columns
            .iter()
            .map(|(name, _)| name.clone())
            .collect();

        let rows_bytes = match stmt.where_clause.clone() {
            None => catalog.scan_rows(meta.root_page, stmt.offset, stmt.limit)?,
            Some(WhereClause::Eq { column, value }) => {
                ensure_pk_filter(&meta, &column)?;
                let mut rows = Vec::new();
                if let Some(bytes) = catalog.get_row(meta.root_page, value)? {
                    rows.push(crate::bptree::KeyValue {
                        key: value,
                        value: bytes,
                    });
                }
                window_rows(rows, stmt.offset, stmt.limit)
            }
            Some(WhereClause::Between { column, from, to }) => {
                ensure_pk_filter(&meta, &column)?;
                let rows = catalog.range_rows(meta.root_page, from, to)?;
                window_rows(rows, stmt.offset, stmt.limit)
            }
        };

        let mut rows = Vec::with_capacity(rows_bytes.len());
        for kv in rows_bytes {
            let decoded = decode_row(&meta, &kv.value)?;
            rows.push(project_row(&selected_columns, &decoded)?);
        }

        Ok(ResultSet {
            columns: output_columns,
            rows,
            message: None,
        })
    }
}

pub fn parse(sql_text: &str) -> DbResult<Vec<Statement>> {
    let mut statements = Vec::new();
    for chunk in split_statements(sql_text) {
        let tokens = tokenize(&chunk)?;
        let mut parser = Parser { tokens, pos: 0 };
        let statement = parser.parse_statement()?;
        if !parser.is_eof() {
            return Err(DbError::new(format!(
                "token inesperado: {}",
                parser.peek().text
            )));
        }
        statements.push(statement);
    }
    Ok(statements)
}

pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let bytes = sql.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let ch = bytes[index] as char;
        if ch == '\'' {
            if in_string && index + 1 < bytes.len() && bytes[index + 1] as char == '\'' {
                current.push_str("''");
                index += 2;
                continue;
            }
            in_string = !in_string;
            current.push(ch);
            index += 1;
            continue;
        }
        if ch == ';' && !in_string {
            let stmt = current.trim();
            if !stmt.is_empty() {
                out.push(stmt.to_string());
            }
            current.clear();
            index += 1;
            continue;
        }
        current.push(ch);
        index += 1;
    }
    let tail = current.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

pub fn encode_row(meta: &TableMeta, values: &HashMap<String, Value>) -> DbResult<(i64, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pk = None;

    for column in &meta.columns {
        let normalized = normalize_ident(&column.name);
        let value = values.get(&normalized).cloned().unwrap_or(Value::Null);
        match (&column.column_type, value) {
            (ColumnType::Int, Value::Null) => {
                if column.name.eq_ignore_ascii_case(&meta.primary_key) {
                    return Err(DbError::new("PRIMARY KEY no puede ser NULL"));
                }
                out.push(0);
            }
            (ColumnType::Int, Value::Integer(number)) => {
                out.push(1);
                out.extend_from_slice(&number.to_le_bytes());
                if column.name.eq_ignore_ascii_case(&meta.primary_key) {
                    pk = Some(number);
                }
            }
            (ColumnType::Float, Value::Null)
            | (ColumnType::Bool, Value::Null)
            | (ColumnType::Text, Value::Null)
            | (ColumnType::Date, Value::Null)
            | (ColumnType::DateTime, Value::Null)
            | (ColumnType::Json, Value::Null) => out.push(0),
            (ColumnType::Float, Value::Float(number)) => {
                out.push(1);
                out.extend_from_slice(&number.to_le_bytes());
            }
            (ColumnType::Float, Value::Integer(number)) => {
                out.push(1);
                out.extend_from_slice(&(number as f64).to_le_bytes());
            }
            (ColumnType::Bool, Value::Bool(flag)) => {
                out.push(1);
                out.push(u8::from(flag));
            }
            (column_type, Value::String(text)) if column_type.stores_as_text() => {
                let bytes = text.as_bytes();
                if bytes.len() > u16::MAX as usize {
                    return Err(DbError::new(format!("{} demasiado largo", column.name)));
                }
                out.push(1);
                out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
                out.extend_from_slice(bytes);
            }
            (ColumnType::Int, _) => {
                return Err(DbError::new(format!("{} debe ser INT", column.name)))
            }
            (ColumnType::Float, _) => {
                return Err(DbError::new(format!("{} debe ser FLOAT", column.name)))
            }
            (ColumnType::Bool, _) => {
                return Err(DbError::new(format!("{} debe ser BOOL", column.name)))
            }
            (_, _) => {
                return Err(DbError::new(format!(
                    "{} debe ser TEXT-compatible",
                    column.name
                )))
            }
        }
    }

    Ok((
        pk.ok_or_else(|| DbError::new("PRIMARY KEY requerida"))?,
        out,
    ))
}

pub fn decode_row(meta: &TableMeta, data: &[u8]) -> DbResult<HashMap<String, Value>> {
    let mut offset = 0usize;
    let mut out = HashMap::new();

    for column in &meta.columns {
        if offset >= data.len() {
            return Err(DbError::new("fila corrupta"));
        }
        let present = data[offset];
        offset += 1;
        let key = normalize_ident(&column.name);
        if present == 0 {
            out.insert(key, Value::Null);
            continue;
        }

        let value = match column.column_type {
            ColumnType::Int => {
                if offset + 8 > data.len() {
                    return Err(DbError::new("fila corrupta (INT)"));
                }
                let number = i64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                Value::Integer(number)
            }
            ColumnType::Float => {
                if offset + 8 > data.len() {
                    return Err(DbError::new("fila corrupta (FLOAT)"));
                }
                let number = f64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                Value::Float(number)
            }
            ColumnType::Bool => {
                if offset >= data.len() {
                    return Err(DbError::new("fila corrupta (BOOL)"));
                }
                let flag = data[offset] != 0;
                offset += 1;
                Value::Bool(flag)
            }
            ColumnType::Text | ColumnType::Date | ColumnType::DateTime | ColumnType::Json => {
                if offset + 2 > data.len() {
                    return Err(DbError::new("fila corrupta (TEXT len)"));
                }
                let len = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                offset += 2;
                if offset + len > data.len() {
                    return Err(DbError::new("fila corrupta (TEXT bytes)"));
                }
                let text = String::from_utf8(data[offset..offset + len].to_vec())?;
                offset += len;
                Value::String(text)
            }
        };
        out.insert(key, value);
    }

    Ok(out)
}

fn project_row(
    selected_columns: &[(String, String)],
    row: &HashMap<String, Value>,
) -> DbResult<Vec<Value>> {
    let mut out = Vec::with_capacity(selected_columns.len());
    for (_, normalized) in selected_columns {
        let value = row.get(normalized).cloned().ok_or_else(|| {
            DbError::new(format!("columna no encontrada en fila: {}", normalized))
        })?;
        out.push(value);
    }
    Ok(out)
}

fn resolve_selected_columns(
    meta: &TableMeta,
    requested: &[String],
) -> DbResult<Vec<(String, String)>> {
    if requested.is_empty() {
        return Ok(meta
            .columns
            .iter()
            .map(|column| (column.name.clone(), normalize_ident(&column.name)))
            .collect());
    }

    let mut out = Vec::with_capacity(requested.len());
    for name in requested {
        let normalized = normalize_ident(name);
        let column = meta
            .column(&normalized)
            .ok_or_else(|| DbError::new(format!("columna no existe: {}", name)))?;
        out.push((column.name.clone(), normalize_ident(&column.name)));
    }
    Ok(out)
}

fn ensure_pk_filter(meta: &TableMeta, column: &str) -> DbResult<()> {
    if meta.primary_key.eq_ignore_ascii_case(column) {
        return Ok(());
    }
    Err(DbError::new(format!(
        "WHERE solo soporta PK ({})",
        meta.primary_key
    )))
}

fn window_rows<T: Clone>(rows: Vec<T>, offset: usize, limit: Option<usize>) -> Vec<T> {
    if offset >= rows.len() {
        return Vec::new();
    }
    let end = match limit {
        Some(limit) => (offset + limit).min(rows.len()),
        None => rows.len(),
    };
    rows[offset..end].to_vec()
}

fn normalize_ident(value: &str) -> String {
    value
        .rsplit('.')
        .next()
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase()
}

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Ident,
    Number,
    String,
    Symbol,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
struct Token {
    kind: TokenKind,
    text: String,
}

fn tokenize(input: &str) -> DbResult<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        if is_ident_start(ch) {
            let start = index;
            index += 1;
            while index < chars.len() && is_ident_part(chars[index]) {
                index += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Ident,
                text: chars[start..index].iter().collect(),
            });
            continue;
        }
        if ch.is_ascii_digit()
            || (ch == '-' && index + 1 < chars.len() && chars[index + 1].is_ascii_digit())
        {
            let start = index;
            index += 1;
            while index < chars.len() && chars[index].is_ascii_digit() {
                index += 1;
            }
            if index < chars.len() && chars[index] == '.' {
                let dot = index;
                index += 1;
                let decimals_start = index;
                while index < chars.len() && chars[index].is_ascii_digit() {
                    index += 1;
                }
                if decimals_start == index {
                    index = dot;
                }
            }
            tokens.push(Token {
                kind: TokenKind::Number,
                text: chars[start..index].iter().collect(),
            });
            continue;
        }
        if ch == '\'' {
            index += 1;
            let mut value = String::new();
            while index < chars.len() {
                if chars[index] == '\'' {
                    if index + 1 < chars.len() && chars[index + 1] == '\'' {
                        value.push('\'');
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                value.push(chars[index]);
                index += 1;
            }
            if index > chars.len() {
                return Err(DbError::new("string sin cierre"));
            }
            tokens.push(Token {
                kind: TokenKind::String,
                text: value,
            });
            continue;
        }
        match ch {
            '(' | ')' | ',' | '*' | '=' => {
                tokens.push(Token {
                    kind: TokenKind::Symbol,
                    text: ch.to_string(),
                });
                index += 1;
            }
            _ => return Err(DbError::new(format!("carÃ¡cter no soportado: {}", ch))),
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        text: String::new(),
    });
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn parse_statement(&mut self) -> DbResult<Statement> {
        if self.match_keyword("CREATE") {
            return self.parse_create();
        }
        if self.match_keyword("INSERT") {
            return self.parse_insert();
        }
        if self.match_keyword("SELECT") {
            return self.parse_select();
        }
        Err(DbError::new(
            "sentencia no soportada (solo CREATE/INSERT/SELECT)",
        ))
    }

    fn parse_create(&mut self) -> DbResult<Statement> {
        self.expect_keyword("TABLE")?;
        let name = self.expect_ident()?;
        self.expect_symbol("(")?;
        let mut columns = Vec::new();
        let mut primary_key = String::new();
        loop {
            let column_name = self.expect_ident()?;
            let type_name = self.expect_ident()?;
            let mut is_pk = false;
            if self.match_keyword("PRIMARY") {
                self.expect_keyword("KEY")?;
                is_pk = true;
                primary_key = column_name.clone();
            }
            columns.push(ColumnDef {
                name: column_name,
                type_name,
                primary_key: is_pk,
            });
            if self.match_symbol(")") {
                break;
            }
            self.expect_symbol(",")?;
        }
        Ok(Statement::CreateTable(CreateTableStmt {
            name,
            columns,
            primary_key,
        }))
    }

    fn parse_insert(&mut self) -> DbResult<Statement> {
        self.expect_keyword("INTO")?;
        let table = self.expect_ident()?;
        self.expect_symbol("(")?;
        let columns = self.parse_ident_list()?;
        self.expect_symbol(")")?;
        self.expect_keyword("VALUES")?;
        self.expect_symbol("(")?;
        let values = self.parse_value_list()?;
        self.expect_symbol(")")?;
        Ok(Statement::Insert(InsertStmt {
            table,
            columns,
            values,
        }))
    }

    fn parse_select(&mut self) -> DbResult<Statement> {
        let columns = if self.match_symbol("*") {
            Vec::new()
        } else {
            self.parse_ident_list()?
        };
        self.expect_keyword("FROM")?;
        let table = self.expect_ident()?;

        let mut where_clause = None;
        if self.match_keyword("WHERE") {
            let column = self.expect_ident()?;
            if self.match_symbol("=") {
                let value = self.expect_integer()?;
                where_clause = Some(WhereClause::Eq { column, value });
            } else if self.match_keyword("BETWEEN") {
                let from = self.expect_integer()?;
                self.expect_keyword("AND")?;
                let to = self.expect_integer()?;
                where_clause = Some(WhereClause::Between { column, from, to });
            } else {
                return Err(DbError::new("WHERE soporta solo '=' o BETWEEN"));
            }
        }

        let mut limit = None;
        let mut offset = 0usize;
        let mut seen_limit = false;
        let mut seen_offset = false;
        loop {
            if self.match_keyword("LIMIT") {
                if seen_limit {
                    return Err(DbError::new("LIMIT repetido"));
                }
                let raw = self.expect_integer()?;
                if raw < 0 {
                    return Err(DbError::new("LIMIT debe ser >= 0"));
                }
                limit = Some(raw as usize);
                seen_limit = true;
                continue;
            }
            if self.match_keyword("OFFSET") {
                if seen_offset {
                    return Err(DbError::new("OFFSET repetido"));
                }
                let raw = self.expect_integer()?;
                if raw < 0 {
                    return Err(DbError::new("OFFSET debe ser >= 0"));
                }
                offset = raw as usize;
                seen_offset = true;
                continue;
            }
            break;
        }

        Ok(Statement::Select(SelectStmt {
            table,
            columns,
            where_clause,
            limit,
            offset,
        }))
    }

    fn parse_ident_list(&mut self) -> DbResult<Vec<String>> {
        let mut out = vec![self.expect_ident()?];
        while self.match_symbol(",") {
            out.push(self.expect_ident()?);
        }
        Ok(out)
    }

    fn parse_value_list(&mut self) -> DbResult<Vec<Value>> {
        let mut out = vec![self.expect_value()?];
        while self.match_symbol(",") {
            out.push(self.expect_value()?);
        }
        Ok(out)
    }

    fn expect_value(&mut self) -> DbResult<Value> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Number => {
                self.pos += 1;
                if token.text.contains('.') {
                    Ok(Value::Float(token.text.parse()?))
                } else {
                    Ok(Value::Integer(token.text.parse()?))
                }
            }
            TokenKind::String => {
                self.pos += 1;
                Ok(Value::String(token.text))
            }
            TokenKind::Ident => {
                if token.text.eq_ignore_ascii_case("TRUE") {
                    self.pos += 1;
                    Ok(Value::Bool(true))
                } else if token.text.eq_ignore_ascii_case("FALSE") {
                    self.pos += 1;
                    Ok(Value::Bool(false))
                } else if token.text.eq_ignore_ascii_case("NULL") {
                    self.pos += 1;
                    Ok(Value::Null)
                } else {
                    Err(DbError::new(format!("valor invÃ¡lido: {}", token.text)))
                }
            }
            _ => Err(DbError::new(format!("valor invÃ¡lido: {}", token.text))),
        }
    }

    fn expect_integer(&mut self) -> DbResult<i64> {
        let token = self.peek().clone();
        if token.kind != TokenKind::Number {
            return Err(DbError::new(format!(
                "se esperaba nÃºmero, llegÃ³: {}",
                token.text
            )));
        }
        if token.text.contains('.') {
            return Err(DbError::new(format!(
                "se esperaba entero, llegÃ³: {}",
                token.text
            )));
        }
        self.pos += 1;
        Ok(token.text.parse()?)
    }

    fn expect_ident(&mut self) -> DbResult<String> {
        let token = self.peek().clone();
        if token.kind != TokenKind::Ident {
            return Err(DbError::new(format!(
                "se esperaba identificador, llegÃ³: {}",
                token.text
            )));
        }
        self.pos += 1;
        Ok(token.text)
    }

    fn expect_keyword(&mut self, expected: &str) -> DbResult<()> {
        if self.match_keyword(expected) {
            return Ok(());
        }
        Err(DbError::new(format!("se esperaba keyword {}", expected)))
    }

    fn match_keyword(&mut self, expected: &str) -> bool {
        let token = self.peek();
        if token.kind == TokenKind::Ident && token.text.eq_ignore_ascii_case(expected) {
            self.pos += 1;
            return true;
        }
        false
    }

    fn expect_symbol(&mut self, expected: &str) -> DbResult<()> {
        if self.match_symbol(expected) {
            return Ok(());
        }
        Err(DbError::new(format!("se esperaba sÃ­mbolo {}", expected)))
    }

    fn match_symbol(&mut self, expected: &str) -> bool {
        let token = self.peek();
        if token.kind == TokenKind::Symbol && token.text == expected {
            self.pos += 1;
            return true;
        }
        false
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn is_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_part(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'
}
