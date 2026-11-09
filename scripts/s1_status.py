#!/usr/bin/env python3
"""S1 dashboard: hours, shell streaks, freezes."""

from __future__ import annotations

import csv
from collections import defaultdict
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
S1 = ROOT / "runtime" / "s1"
HOURS = S1 / "hours.csv"


def main() -> int:
    print("=== S1 STATUS ===")
    print("Rules: no live auto-alpha · skill ≥15h/wk · shell hygiene\n")

    if not HOURS.exists():
        print("No hours.csv yet. Start with:")
        print("  python3 scripts/s1_daily_log.py --hours 2 --track dev --shell --note 'first day'")
        print("Docs: docs/S1_SHELL_INCOME.md")
        return 0

    rows = list(csv.DictReader(HOURS.open()))
    today = date.today()
    week_start = today - timedelta(days=today.weekday())  # Monday
    month_start = today.replace(day=1)

    def in_range(r, start):
        try:
            d = date.fromisoformat(r["date"])
        except Exception:
            return False
        return d >= start

    week = [r for r in rows if in_range(r, week_start)]
    month = [r for r in rows if in_range(r, month_start)]
    last7 = [r for r in rows if in_range(r, today - timedelta(days=6))]

    def sum_h(rs):
        return sum(float(r.get("hours") or 0) for r in rs)

    def shell_n(rs):
        return sum(1 for r in rs if (r.get("shell") or "").lower() in ("yes", "y", "1", "true"))

    wh, mh = sum_h(week), sum_h(month)
    print(f"This week (from {week_start}): {wh:.1f}h skill  |  shell logs: {shell_n(week)}")
    print(f"This month:                {mh:.1f}h skill  |  shell logs: {shell_n(month)}")
    print(f"Last 7 days:               {sum_h(last7):.1f}h  |  shell: {shell_n(last7)}/7")
    print()
    print("Week target: 15h  ", "✓" if wh >= 15 else f"→ need {15-wh:.1f}h more")
    print()

    by = defaultdict(float)
    for r in month:
        by[r.get("track") or "?"] += float(r.get("hours") or 0)
    if by:
        print("Month by track:")
        for k, v in sorted(by.items(), key=lambda x: -x[1]):
            print(f"  {k:12} {v:6.1f}h")
    print()
    print("FROZEN: alpha live · L1 emit live · HB live · new bot deposits")
    print("ON:     shell · journal · skill work")
    print()
    print("Next: python3 scripts/s1_daily_log.py --hours N --track dev --shell")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
