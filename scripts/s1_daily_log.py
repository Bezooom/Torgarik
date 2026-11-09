#!/usr/bin/env python3
"""Append a S1 daily log line and optional journal note."""

from __future__ import annotations

import argparse
import csv
from datetime import date, datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
S1 = ROOT / "runtime" / "s1"
HOURS = S1 / "hours.csv"
DAILY = S1 / "daily.md"


def main() -> int:
    ap = argparse.ArgumentParser(description="S1 daily log")
    ap.add_argument("--hours", type=float, default=0.0, help="skill hours today")
    ap.add_argument("--track", type=str, default="skill", help="dev|data|ops|content|local|other")
    ap.add_argument("--shell", action="store_true", help="mark shell done")
    ap.add_argument("--no-shell", action="store_true", help="mark shell skipped")
    ap.add_argument("--note", type=str, default="")
    ap.add_argument("--mood", type=str, default="", help="optional 1-5 or word")
    args = ap.parse_args()

    S1.mkdir(parents=True, exist_ok=True)
    today = date.today().isoformat()
    shell = "yes" if args.shell or not args.no-shell else "no"
    if args.no_shell:
        shell = "no"
    if args.shell:
        shell = "yes"

    # hours.csv
    new_file = not HOURS.exists()
    with HOURS.open("a", newline="") as f:
        w = csv.writer(f)
        if new_file:
            w.writerow(["date", "hours", "track", "shell", "note", "mood", "logged_at"])
        w.writerow(
            [
                today,
                f"{args.hours:.2f}",
                args.track,
                shell,
                args.note.replace("\n", " ")[:200],
                args.mood,
                datetime.now(timezone.utc).isoformat(),
            ]
        )

    # daily.md append
    if not DAILY.exists():
        DAILY.write_text("# S1 daily log\n\n")
    with DAILY.open("a") as f:
        f.write(
            f"## {today}\n"
            f"- skill hours: **{args.hours}** ({args.track})\n"
            f"- shell: {shell}\n"
            f"- note: {args.note or '—'}\n"
            f"- mood: {args.mood or '—'}\n\n"
        )

    # journal note
    day = datetime.now(timezone.utc).strftime("%Y%m%d")
    jp = ROOT / "runtime" / "journal" / f"journal_{day}.jsonl"
    jp.parent.mkdir(parents=True, exist_ok=True)
    import json

    ev = {
        "kind": "note",
        "ts": datetime.now(timezone.utc).isoformat(),
        "source": "s1_daily",
        "text": f"S1 day hours={args.hours} track={args.track} shell={shell} {args.note}",
    }
    with jp.open("a") as f:
        f.write(json.dumps(ev, ensure_ascii=False) + "\n")

    print(f"logged {today}: {args.hours}h {args.track} shell={shell}")
    print(f"  {HOURS}")
    print(f"  {DAILY}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
