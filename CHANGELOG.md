# Changelog

Все заметные изменения проекта. Формат — [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/),
версии — [SemVer](https://semver.org/lang/ru/).
Подробная история разработки до публикации (фазы 0–6): [docs/CHANGELOG.md](docs/CHANGELOG.md).

## [Unreleased]

## [0.1.0] — 2026-11-09 — первая публичная версия

### Добавлено
- Публичный репозиторий: README на русском с английским резюме, лицензия MIT OR Apache-2.0.
- `docs/RESEARCH.md` и `research/rf_2026_11/` — предрегистрированное исследование 7 идей для РФ
  (фьючерсы и акции Мосбиржи, крипта): правила, хеши, результаты.
- `.env.example`, `config/wallets.example.json` — шаблоны без секретов.
- `CONTRIBUTING.md`, `SECURITY.md`, `.gitleaks.toml`.
- CI: сборка и тесты (блокирующие), `rustfmt`/`clippy` (информативно), сканирование секретов gitleaks.
- Небольшой пример данных `data/history/{ETH,SOL}_1h.csv` для быстрого бэктеста.
- Mermaid-диаграммы в `docs/ARCHITECTURE.md`.

### Изменено
- `rust-version` поднят до 1.88 (этого требуют зависимости из `Cargo.lock`, например `time`).
- Личные пути в документации и скриптах заменены на нейтральные; `scripts/start_sglang.sh`
  теперь берёт модель из `SGLANG_MODEL_PATH`.
- Старый CI заменён: он ссылался на несуществующие Dockerfile и тест `integration`.

### Удалено
- **Устаревшие площадки рынков предсказаний** (Polymarket, Limitless, Azuro):
  клиенты `polymarket_client`, `limitless_client`, `azuro_client`, `gamma_api`,
  `exchange_manager`, EIP-712 подписчик `signer` (Polymarket); варианты
  `VenueKind::Polymarket/Limitless`, `MarketType::Binary/Kalshi`, старые типы
  `Market/Order/Trade/Signal/Decision/...`, поля `polymarket_api_url`, `gamma_api_url`,
  `limitless_api_url` из настроек и `config/*.toml`.
- **Связанные стратегии и модули:** `strategies::crypto_latency` (+ пример `latency_demo`
  и `scripts/latency_demo.sh`), `strategies::fairvalue` (+ MCP-инструмент
  `calculate_fair_value`), неподключённый `ensemble.rs`, в `ml-trader` —
  `signals.rs`, `llm_consensus.rs`, `xgboost_model.rs`; скрипты `train_xgboost.py`, `collect_data.py`.
- **Зависимости:** `ort` (ONNX Runtime), `ndarray` — больше не используются.
- **Крейт `copytrader`** — целиком зависел от клиента Polymarket.
- **`hummingbot/`** и скрипты `m1_setup_hummingbot.sh`, `m1_compare_day.sh`, документ `docs/M1_HUMMINGBOT_PMM.md`.
- **`docs/archive/`** — старые разборы, не актуальны.
- **`tests/e2e`, `tests/integration`** — не принадлежали ни одному крейту и не компилировались.
- Из публикации также исключены: состояние и логи работы (`runtime/`), большие кэши данных
  (кроме примера), списки отслеживаемых кошельков (`data/l1`), личный `config/wallets.json`,
  папки редакторов и агентов (`.kilo/`, `.config/`).

### Статус
- Живая торговля заблокирована предохранителем: `wf_latest.json` — OOS −$466, holdout −$202.
