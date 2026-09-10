"""Validate the generated gabysql system documentation and PDF set."""

from __future__ import annotations

import re
from pathlib import Path

from pypdf import PdfReader

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs" / "system-documentation"


def main() -> None:
    markdown = [DOCS / "README.md", *sorted(DOCS.glob("[0-9][0-9]-*.md"))]
    pdfs = sorted((DOCS / "pdf").glob("*.pdf"))
    assert len(markdown) == 20, f"expected 20 Markdown files, found {len(markdown)}"
    assert len(pdfs) == 20, f"expected 20 PDFs, found {len(pdfs)}"

    missing_links: list[tuple[str, str]] = []
    unbalanced_fences: list[str] = []
    secret_patterns: list[str] = []
    suspicious_secret = re.compile(
        r"(?i)(api[_-]?key|password|secret|token)\s*[:=]\s*['\"]?[A-Za-z0-9_-]{16,}"
    )

    for source in [ROOT / "README.md", *markdown]:
        text = source.read_text(encoding="utf-8")
        assert text.strip(), f"empty document: {source}"
        if text.count("```") % 2:
            unbalanced_fences.append(str(source.relative_to(ROOT)))
        if suspicious_secret.search(text):
            secret_patterns.append(str(source.relative_to(ROOT)))
        for target in re.findall(r"\[[^]]+\]\(([^)]+)\)", text):
            if re.match(r"^[a-z]+://", target) or target.startswith("#"):
                continue
            relative_target = target.split("#", 1)[0]
            if relative_target and not (source.parent / relative_target).resolve().exists():
                missing_links.append((str(source.relative_to(ROOT)), target))

    weak_pages: list[tuple[str, list[int]]] = []
    for pdf in pdfs:
        reader = PdfReader(str(pdf))
        lengths = [len((page.extract_text() or "").strip()) for page in reader.pages]
        if not lengths or any(length < 40 for length in lengths):
            weak_pages.append((pdf.name, lengths))

    assert not missing_links, f"missing links: {missing_links}"
    assert not unbalanced_fences, f"unbalanced code fences: {unbalanced_fences}"
    assert not secret_patterns, f"possible secrets: {secret_patterns}"
    assert not weak_pages, f"blank or weak PDF pages: {weak_pages}"
    total_bytes = sum(pdf.stat().st_size for pdf in pdfs)
    print(
        f"markdown={len(markdown)} pdf={len(pdfs)} links=ok fences=ok "
        f"secrets=none pages_text=ok pdf_bytes={total_bytes}"
    )


if __name__ == "__main__":
    main()
