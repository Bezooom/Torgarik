# L1 — Leaderboard intelligence → filtered signals → risk shell

**Не blind copy.** Пайплайн:

```text
stats-data leaderboard
        │
        ▼
l1_leaderboard.py   →  filter (consistency heuristics)
        │
        ▼
data/l1/watched_wallets.json
        │
        ▼
l1_watch_wallets.py →  poll clearinghouseState
        │
        ▼
runtime/signals.jsonl  →  ml-trader --mode shell  (dry / risk)
```

## 1. Filter leaderboard

```bash
python3 scripts/l1_leaderboard.py --top 30
```

Source: `https://stats-data.hyperliquid.xyz/Mainnet/leaderboard` (public).

Default filters (tune):

| Filter | Default | Why |
|--------|---------|-----|
| account value | $50k–$50M | copyable / not dust / not pure mega-HFT only |
| month PnL | > $5k | not dead |
| month volume | > $1M | active |
| month ROI | < 150% | cut lottery rockets |
| allTime PnL | > 0 | less one-hit month |
| day ≱ 90% of month | — | cut single lucky day |

Output:

- `data/l1/watched_wallets.json`  
- `data/l1/watched_wallets.csv`  
- snapshot with timestamp  

## 2. Watch positions (dry first)

```bash
# one shot (default)
python3 scripts/l1_watch_wallets.py --top 10

# loop, still dry (print only)
python3 scripts/l1_watch_wallets.py --loop --interval 120 --top 10

# emit into shell queue (still paper unless shell --execute live)
python3 scripts/l1_watch_wallets.py --emit --loop --interval 120 --top 10 --stable-polls 2
```

`--stable-polls 2` = side must appear on 2 polls before signal (lag / noise filter).

## 3. Risk shell consumes signals

```bash
# terminal A
cargo run -p ml-trader -- --mode shell

# terminal B
python3 scripts/l1_watch_wallets.py --emit --loop --interval 180 --top 8
```

Shell applies risk caps / kill-switch; dry-run by default.

## 4. What this is / is not

| Is | Is not |
|----|--------|
| Analytics + delayed copy **pipeline** | Proof of edge |
| Separates some one-hit whales | Guarantee of profit |
| Works offline/API read-only for LB | Free money |
| Integrates with your journal | Instant copy of 0x… fill |

**Risks:** lag, capacity, style drift, leaderboard gaming, correlated whales.

## 5. Safety rules

1. Never `--execute` live until dry pipeline runs days without nonsense.  
2. Prefer **side-only** signals (omit size; shell risk %).  
3. Cap wallets (`--top 5–10`).  
4. Re-run `l1_leaderboard.py` daily — rankings rot.  
5. Journal everything.

## 6. Improve later

- Per-wallet realized fill history (`userFills`) for winrate  
- Exclude market makers (huge volume, tiny ROI)  
- Delay N minutes after whale entry  
- Max open copies / coin blacklist  

Related: [lassepaladin/Hyperliquid-Leaderboard-Analytics](https://github.com/lassepaladin/Hyperliquid-Leaderboard-Analytics) (TUI analytics; same public data idea).
