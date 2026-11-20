//! Файл: settings/catalog/dex_lb.rs
//! Что делает: настройки вида Liquidity Book V2.2 (решение 446; lb_v22 — LFJ V2.2 на Arbitrum One): реализации пула,
//! сверенные с исходником, и допущенные крючки обмена. Отдельный файл: каталог dex.rs общий для параллельных задач
//! семейств DEX (решение 433) — так добавление не пересекается с чужими строками. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Контракты DEX»).

use super::{DX, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 446. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_LB: &[SettingDef] = &[
    d("Контракты DEX", "dex.lb_implementations", K::AddressList, L::None, "адреса", DX, "Решение 446: Liquidity Book V2.2 (lb_v22 — LFJ V2.2, id DEX traderjoe-v2-2-arbitrum на Arbitrum One) — адреса реализаций пула LBPair, чей верифицированный код сверен с исходником вида (lfj-gg/joe-v2@v2.2.0 — путь обмена тот же и на main 067c6cc). Клон пула навсегда исполняет свою реализацию, фабрика может сменить реализацию для новых пулов; пул берётся, только если implementation() пула — в списке (pools/verify_lb22.rs). Где взять: записью tools/arbitrum_record/lb_v22.py (implementation() пулов и getLBPairImplementation() фабрики) и сверкой кода реализации (Sourcify / Arbiscan) с исходником. Не задана — DEX не подключена"),
    d("Контракты DEX", "dex.lb_hooks_allowed", K::AddressList, L::None, "адреса", DX, "Решение 446: Liquidity Book V2.2 (lb_v22) — адреса контрактов крючков, которым разрешено стоять на обмене пула (флаги beforeSwap / afterSwap в getLBHooksParameters()). Крючок не меняет выход обмена (зовётся после подсчёта входа, пул закрыт от повторного входа), но это чужой код: он может откатить обмен и тратит газ. Пул без крючка обмена берётся всегда; с крючком обмена — только если адрес крючка в списке; смена крючков пула (событие HooksParametersSet) — пул перечитывается и проверяется заново (pools/lb_hooks.rs). Необязательная. Не задана — пулы с крючком обмена не берутся (причина в журнале dex.pool и на экране DEX)").optional(),
];
