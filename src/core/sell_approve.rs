//! Файл: core/sell_approve.rs
//! Что делает: решение 476 (Тигран 20.11: «approve заранее — да, газ — копейки»; меняет решение 303 «заранее не
//! отправляется»; живой NUM 20.11: первая продажа Gate → DEX ждала approve, его блок и голову опроса — «котировке
//! 2525 мс», потом откат по минимуму) — пока монета сделки идёт на DEX-площадку продажи, разрешения (approve) продажи
//! по ТЕКУЩЕМУ плану отправляются заранее: площадка собирает план продажи, как его поставит исполнение, и отправляет
//! только недостающие сейчас разрешения (контракт dex_preapprove, venues/bsc_dex/exec/preapprove.rs) кошельком
//! сделки.
//! Ревью t238: отдельной задачей (start → tokio::spawn; паника задачи сделку не трогает) — ни перевод, ни продажа её
//! не ждут: approve ещё в полёте — продажа и так встанет в очередь nonce кошелька (wallet/nonce.rs) за ним и увидит
//! выданное разрешение (дубля нет). Итог задачи (газ и строка хронологии) ждёт в памяти по ID сделки и вносится в
//! сделку перед продажей (executor.rs) и при закрытии (ledger::finish) — collect; задача не кончилась — внесёт
//! следующий collect. Объём разрешения — меньшее из продажи плана и купленного (частичная покупка). Ожидание до
//! продажи для срока Permit2 — наибольшее из T плана и transfer.max_time_s (сколько бот ждёт перевод до карантина).
//! Продажу это ни к чему не обязывает: при приходе всё как раньше (sell.rs — выгодно / режим застрявшей монеты /
//! перегон / другая площадка); выбран другой путь или получатель — разрешения перед обменом, как раньше. Газ
//! разрешений — в затраты сделки, как у approve перед обменом (deal_gas.rs, статья «approve заранее»); не удалось —
//! строка в хронологию, продажа сделает approve сама.
//! Кто вызывает: core/executor.rs (start — перевод на B; collect — перед продажей), core/ledger.rs (collect — finish).
//! От чего зависит: precheck_sell.rs (запрос продажи, как у симуляции до покупки), dex_account.rs (кошелёк сделки),
//! trade_ops.rs (вид площадки), deal_gas.rs (Wasted, charge), costs.rs (Calc), deal.rs, engine.rs, settings.
//! Настройки: transfer.max_time_s (чтение; своих нет).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::{CostKind, Direction};
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;

use super::costs::Calc;
use super::deal::Deal;
use super::deal_gas::Wasted;
use super::engine::Core;

/// Итог разрешений заранее: статьи газа (в затраты сделки) и строка хронологии (None — нечего писать).
pub struct Ahead {
    pub gas: Vec<Wasted>,
    pub note: Option<String>,
}

/// Итоги задач по ID сделки, ещё не внесённые в сделку (collect).
type Done = HashMap<String, Ahead>;

static DONE: LazyLock<Mutex<Done>> = LazyLock::new(|| Mutex::new(HashMap::new()));

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

/// Что делает: запустить разрешения заранее отдельной задачей (шапка); площадка продажи — не DEX — ничего. Принимает:
/// ядро, направление, сделку (кошелёк, ID, купленное), план. Ошибок нет.
pub fn start(core: &Core, d: &Direction, deal: &Deal, plan: &Calc) {
    if super::trade_ops::kind(core, &d.sell_on) != Ok(VenueKind::Dex) {
        return;
    }
    let (core, d, deal, plan) = (core.clone(), d.clone(), deal.clone(), plan.clone());
    tokio::spawn(async move {
        let a = Box::pin(ahead(&core, &d, &deal, &plan)).await;
        if a.note.is_some() || !a.gas.is_empty() {
            super::engine::lk(&DONE).insert(deal.id.clone(), a);
        }
    });
}

/// Что делает: внести итог разрешений заранее в сделку (строка хронологии, газ — в затраты), если задача уже кончилась.
/// Принимает: ядро, сделку. Ошибок нет.
pub async fn collect(core: &Core, deal: &mut Deal) {
    let Some(mut a) = super::engine::lk(&DONE).remove(&deal.id) else {
        return;
    };
    if let Some(n) = a.note.take() {
        deal.note(n);
    }
    let v = deal.direction.sell_on.clone();
    super::deal_gas::charge(core, deal, &v, &mut a.gas).await;
}

/// Ожидание до продажи, с: наибольшее из T плана (вверх до секунды) и transfer.max_time_s монеты на площадке
/// покупки (не задана — только T). Ошибок нет.
fn horizon(core: &Core, d: &Direction, plan: &Calc) -> u64 {
    let coin = crate::settings::scope::coin_id(&d.asset.ticker, d.asset.contract.as_ref());
    let at = At {
        venue: Some(&d.buy_on.0),
        coin: Some(&coin),
    };
    let max = core
        .inner
        .deps
        .settings
        .integer("transfer.max_time_s", at)
        .unwrap_or(0);
    plan.transit_s.ceil().to_u64().unwrap_or(0).max(max)
}

/// Что делает: разрешения продажи по плану заранее (шапка). Принимает: ядро, направление, сделку, план. Возвращает:
/// Ahead. Ошибок нет (сбой — строкой).
pub async fn ahead(core: &Core, d: &Direction, deal: &Deal, plan: &Calc) -> Ahead {
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
    // Частичная покупка: купили меньше плана — разрешение на то, что есть.
    if deal.qty > Decimal::ZERO && deal.qty < req.qty {
        req.qty = deal.qty;
    }
    let gw = match super::dex_account::venue_of(core, deal, &d.sell_on).await {
        Ok(g) => g,
        Err(e) => return fail(e),
    };
    match gw.dex_preapprove(&req, horizon(core, d, plan)).await {
        Ok(None) => Ahead {
            gas: vec![],
            note: None,
        },
        Ok(Some(bnb)) => Ahead {
            gas: wasted(Some(bnb)),
            note: Some(format!(
                "approve заранее отправлен и в блоке, пока монета в пути: {} BNB газа (решение 476)",
                bnb.normalize()
            )),
        },
        Err(e) if e.code == ErrorCode::NotApplicable => fail(e.message),
        Err(e) => Ahead {
            gas: wasted(e.approve_gas()),
            note: Some(format!(
                "approve заранее не прошёл: {} — продажа сделает его перед обменом, как раньше (решение 476)",
                e.message
            )),
        },
    }
}
