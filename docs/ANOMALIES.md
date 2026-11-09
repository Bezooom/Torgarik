# Structural anomalies & hypotheses

## Live logger

```bash
cargo run -p ml-trader -- --mode anomaly
```

Пишет в `runtime/journal/`:

| name | смысл |
|------|--------|
| `funding_extreme` | \|funding\| ≥ 0.0002 |
| `funding_flip` | смена знака funding |
| `vol_hot` | короткий mean \|Δmid\| велик |

Это **факты рынка**, не ордера.

## Offline H1 / H2

```bash
python3 scripts/collect_hl_history.py --coins BTC,ETH,SOL --days 60
python3 scripts/analyze_hypotheses.py --coins ETH,SOL,BTC --funding-thr 0.0001
```

### H1 — Funding trap
После экстремального **+funding** (лонги платят) часто ли mid **падает** на 1–24h?  
После **−funding** — растёт?  
Если P(wrong-way) ≫ 50% → trap реален → **veto / fade** (с costs), не «carry forever».

### H2 — Vol expansion
После **тишины** (низкий mean \|ret\|) следующий \|ret\| больше baseline?  
Если нет — breakout-from-quiet слабая идея на 1h.

## H1 deep-dive results (60d, BTC/ETH/SOL, 1440 bars)

| Coin | Funding regime | Trap signal? | Fade toy (next-bar + ~5bps) |
|------|----------------|--------------|------------------------------|
| **ETH** | mostly +1.25e-5 (capped?), rare −funding | weak on +side; −fund sample small | **loss** (mean ret negative) |
| **SOL** | more −funding tails | **one-sided**: after strong −funding, P(ret>0) up to ~70–95% on 8–12h (n=15–22) | thr≈1.9e-5 h=8: n=72 mean≈+0.3% **before** serious OOS |
| **BTC** | very tight around +1.25e-5 | no usable +extreme; few −events | **loss** |

**Critical read:**

1. **HL majors funding is compressed** — often stuck near `1.25e-5` → “extreme” on BTC/ETH almost never happens.  
2. **Classic two-sided funding trap is weak** after costs.  
3. **SOL negative-funding → bounce** looks interesting **but** n small + no purged WF + likely regime-specific → **not** a money machine yet.  
4. Toy fade on **all** high |funding| is **negative** on ETH/BTC once costs applied — matches failed carry OOS.

**Use of H1 in product:**

- Logger threshold lowered to **2.5e-5** (majors-relevant).  
- Manual shell: only consider **SOL** (and later alts) when `funding <= -3e-5`, hold idea 8–12h, size tiny, journal everything.  
- **Do not** auto-fade every funding spike.

Re-run:

```bash
python3 scripts/analyze_hypotheses.py --coins ETH,SOL,BTC --funding-thr 0.00002
```

## Как не превратить в overfit

1. Сначала offline stats на 60d+  
2. Потом logger live 1–2 недели  
3. Только потом — **ручной** signal в shell  
4. Авто-торговля — только после отдельного OOS

Связка: **anomaly → человек → `runtime/signals.jsonl` → risk shell**.