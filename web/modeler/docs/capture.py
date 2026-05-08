"""
Capture script for the gabymodeler user manual.

Drives a real Chromium via Playwright, sets up each documented state
(empty / sample / errors / FK modal / SQL modal / import modal), and
saves a PNG into ./img next to this file.

Run via:
    docker run --rm -v "$(pwd):/work" -w /work --network host \
        mcr.microsoft.com/playwright/python:v1.48.0 python capture.py
"""

import asyncio
import base64
import json
import sys
from pathlib import Path

from playwright.async_api import async_playwright

URL = "http://host.docker.internal:8766/modeler/"
OUT = Path(__file__).parent / "img"
OUT.mkdir(parents=True, exist_ok=True)
VIEWPORT = {"width": 1480, "height": 820}

SAMPLE = {
    "dbName": "shop",
    "ifNotExists": True,
    "entities": [
        {
            "id": "e_users",
            "name": "users",
            "x": 80,
            "y": 80,
            "columns": [
                {"id": "c1", "name": "id",     "type": "INT",  "pk": True,  "notNull": True,  "unique": False, "hasDefault": False, "defaultValue": "", "fk": None},
                {"id": "c2", "name": "email",  "type": "TEXT", "pk": False, "notNull": True,  "unique": True,  "hasDefault": False, "defaultValue": "", "fk": None},
                {"id": "c3", "name": "name",   "type": "TEXT", "pk": False, "notNull": False, "unique": False, "hasDefault": False, "defaultValue": "", "fk": None},
                {"id": "c4", "name": "status", "type": "TEXT", "pk": False, "notNull": True,  "unique": False, "hasDefault": True,  "defaultValue": "pending", "fk": None},
            ],
        },
        {
            "id": "e_orders",
            "name": "orders",
            "x": 480,
            "y": 80,
            "columns": [
                {"id": "c5", "name": "id",      "type": "INT",   "pk": True,  "notNull": True,  "unique": False, "hasDefault": False, "defaultValue": "", "fk": None},
                {"id": "c6", "name": "user_id", "type": "INT",   "pk": False, "notNull": False, "unique": False, "hasDefault": False, "defaultValue": "",
                 "fk": {"table": "users", "column": "id", "onDelete": "CASCADE"}},
                {"id": "c7", "name": "total",   "type": "FLOAT", "pk": False, "notNull": False, "unique": False, "hasDefault": False, "defaultValue": "", "fk": None},
                {"id": "c8", "name": "tries",   "type": "INT",   "pk": False, "notNull": False, "unique": False, "hasDefault": True,  "defaultValue": 0,  "fk": None},
            ],
        },
    ],
}

# Variation that triggers two Check Model errors: reserved word + broken FK.
BROKEN = json.loads(json.dumps(SAMPLE))
BROKEN["entities"][0]["name"] = "select"


async def set_state(page, state):
    await page.evaluate(
        "(s) => { localStorage.setItem('gabymodeler.v2', JSON.stringify(s)); }",
        state,
    )
    await page.reload(wait_until="domcontentloaded")
    await page.wait_for_timeout(400)


async def shot(page, name):
    target = OUT / f"{name}.png"
    await page.screenshot(path=str(target), full_page=False)
    print(f"  saved {target.name} ({target.stat().st_size // 1024} KB)")


async def main():
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        ctx = await browser.new_context(viewport=VIEWPORT, device_scale_factor=1)
        page = await ctx.new_page()

        # 1) Empty state.
        await page.goto(URL, wait_until="domcontentloaded")
        await page.evaluate("() => localStorage.removeItem('gabymodeler.v2')")
        await page.reload(wait_until="domcontentloaded")
        await page.wait_for_timeout(300)
        await shot(page, "01-empty")

        # 2) Sample loaded.
        await set_state(page, SAMPLE)
        await shot(page, "02-sample")

        # 3) SQL Preview tab on the sample.
        await page.click("text=SQL Preview")
        await page.wait_for_timeout(200)
        await shot(page, "03-sql-preview")

        # 4) Check Model with two errors (rename + broken FK cascade).
        await set_state(page, BROKEN)
        await page.click("text=Check Model")
        await page.wait_for_timeout(200)
        await shot(page, "04-check-errors")

        # 5) FK modal — open via the FK flag on orders.user_id.
        await set_state(page, SAMPLE)
        # Click FK flag of the second column inside the orders entity
        # (the user_id column).
        fk_handles = await page.locator(".colflag.fk").all()
        # In SAMPLE, orders is the second card; we want its user_id row's
        # FK flag, which is fk_handles[5] (4 in users + then orders.id then orders.user_id at idx 5).
        # To be robust, click the one that's already on (highlighted).
        for h in fk_handles:
            cls = await h.get_attribute("class")
            if cls and " on" in cls:
                await h.click()
                break
        await page.wait_for_timeout(300)
        await shot(page, "05-fk-modal")
        await page.keyboard.press("Escape")
        await page.click("[data-close=modal-fk]", timeout=2000)
        await page.wait_for_timeout(200)

        # 6) Ver SQL modal.
        await page.click("#btn-export")
        await page.wait_for_timeout(300)
        await shot(page, "06-sql-modal")
        await page.click("[data-close=modal-sql]")
        await page.wait_for_timeout(200)

        # 7) Importar modal.
        await page.click("#btn-import")
        await page.wait_for_timeout(300)
        await shot(page, "07-import-modal")

        await browser.close()


if __name__ == "__main__":
    asyncio.run(main())
