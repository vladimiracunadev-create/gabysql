"""Generate PDF copies of docs/system-documentation Markdown files."""

from __future__ import annotations

import re
import sys
from pathlib import Path

from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.lib.units import mm
from reportlab.platypus import BaseDocTemplate, Frame, ListFlowable, ListItem, PageTemplate, Paragraph, Preformatted, Spacer, Table, TableStyle

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs" / "system-documentation"
OUTPUT = DOCS / "pdf"
COMMIT = "ed4135c"
ANALYSIS_DATE = "10 de septiembre de 2026"


def inline(text: str) -> str:
    """Escape XML while retaining simple code and link markup."""
    links: list[tuple[str, str]] = []
    codes: list[str] = []
    text = re.sub(r"!\[([^]]*)\]\([^)]+\)", r"\1", text)

    def save_link(match: re.Match[str]) -> str:
        links.append((match.group(1), match.group(2)))
        return f"@@LINK{len(links)-1}@@"

    def save_code(match: re.Match[str]) -> str:
        codes.append(match.group(1))
        return f"@@CODE{len(codes)-1}@@"

    text = re.sub(r"\[([^]]+)\]\(([^)]+)\)", save_link, text)
    text = re.sub(r"`([^`]+)`", save_code, text)
    text = text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    text = re.sub(r"\*\*([^*]+)\*\*", r"<b>\1</b>", text)
    for index, (label, target) in enumerate(links):
        text = text.replace(f"@@LINK{index}@@", f'<link href="{target}">{label}</link>')
    for index, code in enumerate(codes):
        text = text.replace(f"@@CODE{index}@@", f'<font name="Courier">{code}</font>')
    return text


STYLES = getSampleStyleSheet()
STYLES.add(ParagraphStyle(name="TitleX", parent=STYLES["Title"], fontName="Helvetica-Bold", fontSize=23, leading=28, textColor=colors.HexColor("#14213D"), spaceAfter=9))
STYLES.add(ParagraphStyle(name="MetaX", parent=STYLES["Normal"], fontSize=8.2, leading=10, textColor=colors.HexColor("#5E6B78"), spaceAfter=12))
for level, size in ((1, 17), (2, 13), (3, 11)):
    STYLES.add(ParagraphStyle(name=f"HX{level}", parent=STYLES[f"Heading{level}"], fontName="Helvetica-Bold", fontSize=size, leading=size+4, textColor=colors.HexColor("#2463A8"), spaceBefore=9, spaceAfter=5, keepWithNext=True))
STYLES.add(ParagraphStyle(name="BodyX", parent=STYLES["BodyText"], fontName="Helvetica", fontSize=8.8, leading=12.5, textColor=colors.HexColor("#202833"), spaceAfter=5))
STYLES.add(ParagraphStyle(name="SmallX", parent=STYLES["BodyText"], fontSize=6.9, leading=8.8, textColor=colors.HexColor("#202833")))
STYLES.add(ParagraphStyle(name="CodeX", parent=STYLES["Code"], fontName="Courier", fontSize=6.8, leading=8.6, borderColor=colors.HexColor("#D6DEE8"), borderWidth=.5, borderPadding=5, backColor=colors.HexColor("#F4F7FA"), spaceBefore=3, spaceAfter=7))


class Document(BaseDocTemplate):
    def __init__(self, path: Path, title: str):
        super().__init__(str(path), pagesize=A4, leftMargin=18*mm, rightMargin=18*mm, topMargin=19*mm, bottomMargin=18*mm, title=title, author="gabysql project")
        self.addPageTemplates(PageTemplate(id="main", frames=Frame(self.leftMargin, self.bottomMargin, self.width, self.height, id="body"), onPage=self.decorate))

    @staticmethod
    def decorate(canvas, doc):
        canvas.saveState()
        canvas.setStrokeColor(colors.HexColor("#D6DEE8")); canvas.setLineWidth(.4)
        canvas.line(doc.leftMargin, A4[1]-12*mm, A4[0]-doc.rightMargin, A4[1]-12*mm)
        canvas.setFont("Helvetica", 7.3); canvas.setFillColor(colors.HexColor("#5E6B78"))
        canvas.drawString(doc.leftMargin, A4[1]-9*mm, "gabysql - documentación del sistema")
        canvas.drawRightString(A4[0]-doc.rightMargin, 9*mm, f"{COMMIT}  |  página {doc.page}")
        canvas.restoreState()


def make_table(lines: list[str]) -> Table:
    rows = [[cell.strip() for cell in row.strip("|").split("|")] for row in lines]
    if len(rows) > 1 and all(re.fullmatch(r":?-{3,}:?", cell) for cell in rows[1]):
        rows.pop(1)
    data = [[Paragraph(inline(cell), STYLES["SmallX"]) for cell in row] for row in rows]
    widths = [174*mm/max(len(row) for row in data)] * max(len(row) for row in data)
    result = Table(data, colWidths=widths, repeatRows=1, hAlign="LEFT")
    result.setStyle(TableStyle([("BACKGROUND", (0,0), (-1,0), colors.HexColor("#DCE9F7")), ("GRID", (0,0), (-1,-1), .35, colors.HexColor("#B7C4D3")), ("VALIGN", (0,0), (-1,-1), "TOP"), ("LEFTPADDING", (0,0), (-1,-1), 4), ("RIGHTPADDING", (0,0), (-1,-1), 4), ("TOPPADDING", (0,0), (-1,-1), 3), ("BOTTOMPADDING", (0,0), (-1,-1), 3)]))
    return result


def markdown_flowables(text: str) -> list:
    out: list = []
    paragraph: list[str] = []
    bullets: list[str] = []
    table: list[str] = []
    code: list[str] = []
    in_code = False

    def flush():
        if paragraph:
            out.append(Paragraph(inline(" ".join(paragraph)), STYLES["BodyX"])); paragraph.clear()
        if bullets:
            out.append(ListFlowable([ListItem(Paragraph(inline(item), STYLES["BodyX"])) for item in bullets], bulletType="bullet", leftIndent=16, spaceAfter=5)); bullets.clear()
        if table:
            out.extend((make_table(table), Spacer(1, 6))); table.clear()

    for raw in text.splitlines():
        line = raw.rstrip()
        if line.startswith("```"):
            flush()
            if in_code:
                out.append(Preformatted("\n".join(code), STYLES["CodeX"])); code.clear()
            in_code = not in_code
            continue
        if in_code:
            code.append(line); continue
        if line.startswith("|") and line.endswith("|"):
            if paragraph or bullets:
                flush()
            table.append(line); continue
        if table:
            flush()
        heading = re.match(r"^(#{1,3})\s+(.+)$", line)
        if heading:
            flush(); out.append(Paragraph(inline(heading.group(2)), STYLES[f"HX{len(heading.group(1))}"])); continue
        if re.match(r"^(?:[-*]|\d+\.)\s+", line):
            if paragraph:
                flush()
            bullets.append(re.sub(r"^(?:[-*]|\d+\.)\s+", "", line)); continue
        if not line.strip():
            flush(); continue
        if line.startswith(">"):
            flush(); out.append(Paragraph(inline(line.lstrip("> ")), STYLES["MetaX"])); continue
        paragraph.append(line.strip())
    flush()
    if code:
        out.append(Preformatted("\n".join(code), STYLES["CodeX"]))
    return out


def main() -> int:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    sources = [DOCS / "README.md", *sorted(DOCS.glob("[0-9][0-9]-*.md"))]
    if len(sources) != 20:
        raise SystemExit(f"expected 20 Markdown sources, found {len(sources)}")
    for source in sources:
        content = source.read_text(encoding="utf-8")
        title = next(line[2:] for line in content.splitlines() if line.startswith("# "))
        body = re.sub(r"^# .+?\r?\n", "", content, count=1)
        story = [Paragraph(inline(title), STYLES["TitleX"]), Paragraph(f"gabysql 0.2.0 | commit {COMMIT} | {ANALYSIS_DATE} | Fuente: {source.name}", STYLES["MetaX"]), *markdown_flowables(body)]
        target = OUTPUT / f"{source.stem}.pdf"
        Document(target, title).build(story)
        print(target.relative_to(ROOT))
    return 0


if __name__ == "__main__":
    sys.exit(main())
