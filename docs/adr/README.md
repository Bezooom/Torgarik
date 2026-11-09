# Architecture Decision Records

Короткие записи о решениях, которые дорого отменять. Формат: Status, Context,
Decision, Consequences, Verification.

| # | Решение | Статус |
|---|---|---|
| [0001](0001-risk-shell-over-auto-alpha.md) | Щит и ручной альфа-канал вместо автоторговли | Accepted |
| [0002](0002-oos-kill-switch.md) | OOS-гейт блокирует live в коде, а не в инструкции | Accepted |
| [0003](0003-point-in-time-causality-audit.md) | Look-ahead ловится свойством, а не обзором | Accepted |
| [0004](0004-selection-bias-gate.md) | DSR и PBO участвуют в kill-switch | Accepted |
| [0005](0005-marketdata-vs-exchange.md) | Чтение отделено от торговли на уровне типов | Accepted |
| [0006](0006-rf-tax-in-common.md) | Налоги и лоты живут в доменном крейте | Accepted |
| [0007](0007-single-signal-engine.md) | Один генератор сигналов для бэктеста и shell | Accepted |
| [0008](0008-remote-stop-and-telegram.md) | Остановка файлом, Telegram только читает и стопит | Accepted |

## Когда писать ADR

- решение меняет контракт между крейтами;
- решение ограничивает будущие варианты (нельзя «просто добавить»);
- через полгода кто-то спросит «почему не наоборот».

ADR не описывает, как код устроен. Это делает [ARCHITECTURE.md](../ARCHITECTURE.md).
