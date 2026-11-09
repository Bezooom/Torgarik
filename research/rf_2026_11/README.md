# RF research, November 2026

Pre-registered study of 7 hypotheses (plus variants) for a trader based in Russia.
Summary and interpretation: [docs/RESEARCH.md](../../docs/RESEARCH.md).

| File | What |
|---|---|
| `prereg.md`, `prereg.sha256` | Round 1 rules (H1–H4), written before any result was computed |
| `prereg2.md`, `prereg2.sha256` | Round 2 rules (C1–C3, crypto) |
| `final.json` | Round 1 metrics per period + scores |
| `res_round2.json` | Round 2 metrics per period + scores |
| `scripts/` | Quick research scripts (Python, pandas, numpy, requests) |

Verify the pre-registration: `sha256sum -c prereg.sha256 prereg2.sha256`.

Reproduce (approximately): run `scripts/fetch_*.py` / `scripts/fetch2*.py` from this
directory to download MOEX ISS and Binance data into `./data/` (~130 MB, not committed),
then `h1.py`, `h2.py`, `h3h4.py`, `c123.py`. The scripts are research-grade, not production code.
Data sources: MOEX ISS (iss.moex.com), Binance public data (data.binance.vision) and REST.
