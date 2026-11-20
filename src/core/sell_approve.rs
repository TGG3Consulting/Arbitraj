//! Файл: core/sell_approve.rs
//! Что делает: решение 476 (Тигран 20.11: «approve заранее — да, газ — копейки»; меняет решение 303 «заранее не
//! отправляется»; живой NUM 20.11: первая продажа Gate → DEX ждала approve, его блок и голову опроса — «котировке
//! 2525 мс», потом откат по минимуму) — пока монета сделки идёт на DEX-площадку продажи, разрешения (approve) продажи
//! по ТЕКУЩЕМУ плану отправляются заранее: площадка собирает план продажи, как его поставит исполнение, и отправляет
//! только недостающие сейчас разрешения (контракт dex_preapprove, venues/bsc_dex/exec/preapprove.rs) кошельком
//! сделки. Идёт ОДНОВРЕМЕННО с переводом (executor.rs: tokio::join с transfers::send_trade) — перевод его не ждёт.
//! Продажу это ни к чему не обязывает: при приходе всё как раньше (sell.rs — выгодно / режим застрявшей монеты /
//! перегон / другая площадка); выбран другой путь или получатель — разрешения перед обменом, как раньше. Газ
//! разрешений — в затраты сделки, как у approve перед обменом (deal_gas.rs, статья «approve заранее»); не удалось —
//! строка в хронологию, продажа сделает approve сама.
//! Кто вызывает: core/executor.rs (перевод на B).
//! От чего зависит: precheck_sell.rs (запрос продажи, как у симуляции до покупки), dex_account.rs (кошелёк сделки),
//! trade_ops.rs (вид площадки), deal_gas.rs (Wasted), costs.rs (Calc), deal.rs, engine.rs.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::{CostKind, Direction};
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;

use super::costs::Calc;
use super::deal::Deal;
use super::deal_gas::Wasted;
use super::engine::Core;

/// Итог разрешений заранее: статьи газа (в затраты сделки) и строка хронологии (None — нечего писать).
pub struct Ahead {
    pub gas: Vec<Wasted>,
    pub note: Option<String>,
}

/// Статья газа «approve заранее». Ошибок нет.
fn wasted(bnb: Option<Decimal>) -> Vec<Wasted> {
    bnb.filter(|b| !b.is_zero())
        .map(|bnb| Wasted {
            what: "approve заранее".into(),
            kind: CostKind::ApproveGas,
            bnb,
        })
        .into_iter()
        .collect()
}

/// Что делает: разрешения продажи по плану заранее, пока монета в пути (шапка). Площадка продажи — не DEX — ничего.
/// Принимает: ядро, направление, сделку (её кошелёк и ID), план. Возвращает: Ahead. Ошибок нет (сбой — строкой).
pub async fn ahead(core: &Core, d: &Direction, deal: &Deal, plan: &Calc) -> Ahead {
    let none = Ahead {
        gas: vec![],
        note: None,
    };
    if super::trade_ops::kind(core, &d.sell_on) != Ok(VenueKind::Dex) {
        return none;
    }
    let fail = |why: String| Ahead {
        gas: vec![],
        note: Some(format!(
            "approve заранее не отправлен: {why} — продажа сделает его перед обменом, как раньше (решение 476)"
        )),
    };
    let mut req = match super::precheck_sell::request(core, d, plan) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    req.deal_id = deal.id.clone();
    let gw = match super::dex_account::venue_of(core, deal, &d.sell_on).await {
        Ok(g) => g,
        Err(e) => return fail(e),
    };
    match gw.dex_preapprove(&req).await {
        Ok(None) => none,
        Ok(Some(bnb)) => Ahead {
            gas: wasted(Some(bnb)),
            note: Some(format!(
                "approve заранее отправлен и в блоке, пока монета в пути: {} BNB газа (решение 476)",
                bnb.normalize()
            )),
        },
        Err(e) if e.code == ErrorCode::NotApplicable => none,
        Err(e) => Ahead {
            gas: wasted(e.approve_gas()),
            note: Some(format!(
                "approve заранее не прошёл: {} — продажа сделает его перед обменом, как раньше (решение 476)",
                e.message
            )),
        },
    }
}
