//! Файл: core/credit_shortfall.rs
//! Что делает: страховка от неизмеренного налога перевода (решение 118, Я-3): перевод зачислен, а дошло меньше
//! ожидаемого (вывод минус комиссия вывода и известный налог — credit_expect::arrival, как в расчёте) больше чем на
//! transfer.credit_shortfall_pct — монета уходит в ручной чёрный список (coin.blocked = да, coin.blocked_note —
//! «зачислено меньше на N% — похоже на налог перевода»), журнал и уведомление coin.credit_shortfall. Больше эта монета
//! не торгуется, пока человек не снимет отметку на экране «Монеты». USDT (перегон) не проверяется — его блок
//! остановил бы всю торговлю, а налога у него нет.
//! Кто вызывает: core/transfers.rs (после зачисления).
//! От чего зависит: credit_expect.rs, engine.rs, settings (запись ручной отметки), deps.rs (журнал, уведомления).
//! Настройки: transfer.credit_shortfall_pct (вся система, необязательная; не задана — без авто-блока);
//! пишет coin.blocked и coin.blocked_note (по монете — тикер, как ручной список).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, rd};

/// Кто меняет настройку (журнал изменений настроек).
const AUTHOR: &str = "ядро: зачислено меньше ожидаемого (решение 118)";

/// Сверяет зачисленное с ожидаемым и при недостаче больше допуска блокирует монету. Принимает: сделку, откуда,
/// куда, монету, сеть отправителя, сумму вывода, зачислено. Ошибок нет: ожидание не посчитать — проверки нет.
#[allow(clippy::too_many_arguments)]
pub async fn check(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    net: &str,
    sent: Decimal,
    credited: Decimal,
) {
    let deps = &core.inner.deps;
    let Ok(max) = deps
        .settings
        .decimal("transfer.credit_shortfall_pct", At::default())
    else {
        return;
    };
    if rd(&core.inner.usdt).as_ref().ok() == asset.bsc_contract.as_ref() {
        return;
    }
    let Ok(expected) = super::credit_expect::arrival(core, from, to, asset, net, sent).await else {
        return;
    };
    let Some(pct) = shortfall_pct(expected, credited) else {
        return;
    };
    if pct <= max {
        return;
    }
    let pct = pct.round_dp(4);
    let note = format!(
        "зачислено меньше на {pct}% — похоже на налог перевода ({deal}: {} → {}, ожидалось {expected}, пришло {credited})",
        from.0, to.0
    );
    let scope = super::blacklist::coin_scope(asset);
    let s = &deps.settings;
    let set = async {
        s.set(
            "coin.blocked_note",
            scope.clone(),
            Some(Value::String(note.clone())),
            AUTHOR,
        )
        .await?;
        s.set("coin.blocked", scope, Some(Value::Bool(true)), AUTHOR)
            .await
    };
    let text = match set.await {
        Ok(_) => format!("{}: в ручном чёрном списке — {note}", asset.ticker),
        Err(e) => format!(
            "{}: {note}; в чёрный список не внесена ({e}) — внесите вручную",
            asset.ticker
        ),
    };
    deps.log(
        Level::Warn,
        "core.credit_shortfall",
        Some(&to.0),
        &text,
        None,
    )
    .await;
    deps.notify("coin.credit_shortfall", Some(&to.0), &text)
        .await;
}

/// Недостача зачисления, % от ожидаемого. None — недостачи нет или ожидание не больше нуля. Ошибок нет.
pub fn shortfall_pct(expected: Decimal, credited: Decimal) -> Option<Decimal> {
    (expected > Decimal::ZERO && credited < expected)
        .then(|| (expected - credited) / expected * Decimal::ONE_HUNDRED)
}
