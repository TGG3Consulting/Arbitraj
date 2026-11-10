//! Файл: core/usdt_return_deal.rs
//! Что делает: общий шаг «после закрытия — возврат USDT на площадку покупки» (решение 138). Вызывается из ОДНОГО места —
//! ledger::finish при Step::Closed, поэтому ни один путь закрытия (обычная продажа, застрявшая монета и «Перегон» —
//! продано здесь или на третьей площадке, «Продать сейчас», «Продать и перегнать», «Вернуть боту», «Продано вручную»,
//! пересчёт до покупки) не может его пропустить. Возврат нужен — режим usdt_return.mode = per_trade или флаг сделки
//! return_usdt («Продать и перегнать», решение 101) — и выручка не на площадке покупки (deal.at ≠ buy_on).
//! Перегон — отдельная задача класса Trade (путь сделки, решение 61: раньше задача шла фоном — классом Background в
//! очереди узла BSC); итог — в состояние сделки (usdt_return_state.rs). Возврата нет и не будет (сбой, нет кошелька,
//! задача оборвалась, исключённый путь — Deal.usdt_return_skip) — страховка «USDT не вернулись».
//! Решение 224: исключённый путь («Продано вручную», решение 100) запрещает перегон только выручки человека — выручку
//! продаж БОТА в той же сделке (deal.sells на площадке выручки) бот перегоняет сам, как при обычном закрытии.
//! Решение 227: продажи на нескольких площадках (живой случай JCT: часть на Bitget, остаток на DEX) — возврат по
//! частям: с каждой площадки выручки ровно её выручка минус комиссия, каждая часть — своим перегоном (Job), состояние —
//! по частям (usdt_return_parts.rs; части считает usdt_return_split.rs). Одна площадка продаж — как раньше.
//! Кто вызывает: core/ledger.rs (finish).
//! От чего зависит: usdt_return_job.rs (запуск перегона), usdt_return_state.rs, usdt_return_parts.rs,
//! usdt_return_split.rs, dex_account.rs, deal.rs, settings.
//! Решение 249: часть биржа → биржа, запрещённая venue.cex_cex (cex_link.rs), не перегоняется — страховка «USDT не вернулись».
//! Настройки: usdt_return.mode, venue.quote_asset; venue.cex_cex (через cex_link.rs).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::usdt_return_state::{self as st, UsdtReturn};

pub use super::usdt_return_job::Job;

/// Нужен ли возврат этой закрытой сделке: per_trade или «Продать и перегнать». Ошибок нет: режим не задан — нет.
pub fn wanted(core: &Core, deal: &Deal) -> bool {
    let mode = core
        .inner
        .deps
        .settings
        .text("usdt_return.mode", At::default());
    // Варианты настройки — per_trade / separate (settings/catalog/trade.rs).
    deal.return_usdt || mode.ok().as_deref() == Some("per_trade")
}

/// Решает возврат закрытой сделки и ставит состояние в сделку (сохранит ledger::finish). Решение 227: продажи на
/// нескольких площадках — часть с каждой площадки выручки (кроме площадки покупки), каждая своим перегоном.
/// Принимает: ядро, сделку. Возвращает: перегоны — запустить после сохранения; пусто — возврат не нужен или уже решён.
/// Ошибок нет: сбой — страховка.
pub async fn prepare(core: &Core, deal: &mut Deal) -> Vec<Job> {
    if deal.usdt_return.is_some() || !wanted(core, deal) {
        return vec![];
    }
    let parts = if super::usdt_return_parts::multi(deal) {
        super::usdt_return_split::parts(core, deal).await
    } else {
        single(core, deal).await.into_iter().collect()
    };
    if parts.is_empty() {
        return vec![];
    }
    // Решение 132: перегон выручки — кошельком сделки.
    let acc = match super::dex_account::of(core, deal) {
        Ok(a) => a,
        Err(e) => {
            for (from, amount) in &parts {
                not_returned(core, deal, *amount, from, &format!("перегон не начат: {e}")).await;
            }
            return vec![];
        }
    };
    let to = deal.direction.buy_on.clone();
    let n = parts.len();
    let mut jobs = Vec::with_capacity(n);
    for (i, (from, amount)) in parts.into_iter().enumerate() {
        // Решение 249: перегон USDT биржа → биржа запрещён venue.cex_cex (сменилась после начала сделки) — без перегона,
        // страховка «USDT не вернулись», решает человек.
        if let Some(why) = return_block(core, &from, &to) {
            not_returned(core, deal, amount, &from, &why).await;
            continue;
        }
        let of = if n > 1 {
            format!(" (часть {} из {n}, решение 227)", i + 1)
        } else {
            String::new()
        };
        let text = format!(
            "Сделка {}: возврат USDT {amount}: {} → {} начат{of}",
            deal.id, from.0, to.0
        );
        deal.note(text.clone());
        core.inner
            .deps
            .log(Level::Info, "core.usdt_return", Some(&from.0), text, None)
            .await;
        let state = UsdtReturn::Pending {
            amount,
            from: from.clone(),
            to: to.clone(),
        };
        super::usdt_return_parts::put(deal, state);
        jobs.push(Job {
            deal: deal.id.clone(),
            amount,
            from,
            to: to.clone(),
            acc: acc.clone(),
        });
    }
    jobs
}

/// Одна часть — все продажи на площадке закрытия (как до решения 227). Принимает: ядро, сделку. Возвращает: (откуда,
/// сумма) или None — возврат не нужен или не будет (страховка уже поставлена). Ошибок нет.
async fn single(core: &Core, deal: &mut Deal) -> Option<(VenueId, Decimal)> {
    let deps = &core.inner.deps;
    if deal.at == deal.direction.buy_on {
        // Решение 108: продано там же, где куплено (монета не уезжала) — выручка уже на площадке покупки.
        if deal.return_usdt {
            no_need(core, deal).await;
        }
        return None;
    }
    // Решение 118 (Я-22): уходит то, что пришло: выручка минус комиссия продажи в котируемой монете площадки.
    let quote = deps.settings.text("venue.quote_asset", at(&deal.at));
    let mut amount = match &quote {
        Ok(q) => deal.proceeds - sell_fee(deal, q),
        Err(_) => deal.proceeds,
    };
    if amount <= Decimal::ZERO {
        return None;
    }
    let from = deal.at.clone();
    // Исключённый путь (например «Продано вручную», решение 100): выручку человека бот не перегоняет. Решение 224:
    // выручку продаж бота в этой сделке — перегоняет (не больше общей суммы); продаж бота нет — страховка, как раньше.
    if let Some(why) = deal.usdt_return_skip.clone() {
        let bot = bot_part(deal, quote.as_deref().ok()).min(amount);
        if bot <= Decimal::ZERO {
            not_returned(core, deal, amount, &from, &why).await;
            return None;
        }
        let human = amount - bot;
        if human > Decimal::ZERO {
            deal.note(format!(
                "Сделка {}: выручку человека {human} USDT бот не перегоняет ({why}); перегоняется выручка продаж бота {bot} USDT (решение 224)",
                deal.id
            ));
        }
        amount = bot;
    }
    Some((from, amount))
}

/// Решение 249: перегон выручки с биржи `from` на биржу покупки `to` — правило venue.cex_cex обеих (в обе стороны). Список
/// venue.cex_cex_deny биржи покупки возврат на неё саму не запрещает (запрет — только уход с неё). Одна из сторон DEX —
/// без условия. Принимает: ядро, откуда, куда. Возвращает: причину запрета или None.
fn return_block(core: &Core, from: &VenueId, to: &VenueId) -> Option<String> {
    let cex = |v: &VenueId| {
        super::trade_ops::kind(core, v).ok() == Some(crate::gateway::contract::VenueKind::Cex)
    };
    if from == to || !cex(from) || !cex(to) {
        return None;
    }
    super::cex_link::flags(&core.inner.deps.settings, &from.0, &to.0)
        .map(|w| format!("перегон USDT {} → {} запрещён: {w}", from.0, to.0))
}

/// Журнал «перегон не нужен — выручка уже на площадке покупки» (решение 108, «Продать и перегнать»). Ошибок нет.
pub(super) async fn no_need(core: &Core, deal: &Deal) {
    let text = format!(
        "Сделка {}: перегон не нужен — продано на {}, выручка уже на площадке покупки",
        deal.id, deal.at.0
    );
    core.inner
        .deps
        .log(
            Level::Info,
            "core.usdt_return",
            Some(&deal.at.0),
            text,
            None,
        )
        .await;
}

/// Возврата этой части не будет: состояние части в сделку, страховка. Ошибок нет.
pub(super) async fn not_returned(
    core: &Core,
    deal: &mut Deal,
    amount: Decimal,
    from: &VenueId,
    why: &str,
) {
    deal.note(st::not_returned_text(&deal.id, amount, from, why));
    let state = UsdtReturn::NotReturned {
        amount,
        at: from.clone(),
        why: why.to_string(),
    };
    super::usdt_return_parts::put(deal, state);
    st::alarm(core, &deal.id, amount, from, why).await;
}

/// Комиссия продаж сделки в котируемой монете (её нет в выручке). Принимает: сделку, тикер котируемой. Ошибок нет.
fn sell_fee(deal: &Deal, quote: &str) -> Decimal {
    deal.sells.iter().map(|r| r.fee_in(quote)).sum()
}

/// Решение 224: выручка продаж БОТА (deal.sells) на площадке, где лежит выручка (deal.at), до комиссии. Ручной
/// ввод «Продано вручную» в sells не попадает. Принимает: сделку. Возвращает: сумму USDT (0 — бот здесь не продавал).
/// Ошибок нет.
pub fn bot_sold(deal: &Deal) -> Decimal {
    super::usdt_return_parts::sold_at(deal, &deal.at)
}

/// Решение 224: сколько выручки продаж бота можно вернуть — bot_sold минус комиссия этих продаж в котируемой монете.
/// Принимает: сделку, тикер котируемой (None — не задан: без вычета). Возвращает: сумму (может быть ≤ 0). Ошибок нет.
pub fn bot_part(deal: &Deal, quote: Option<&str>) -> Decimal {
    super::usdt_return_parts::amount_at(deal, &deal.at, quote)
}

/// Решение 227: сумма части с площадки `v` — выручка продаж бота там минус их комиссия в котируемой монете площадки
/// (настройка venue.quote_asset). Принимает: ядро, сделку, площадку. Ошибок нет.
pub fn amount_of(core: &Core, deal: &Deal, v: &VenueId) -> Decimal {
    let q = core.inner.deps.settings.text("venue.quote_asset", at(v));
    super::usdt_return_parts::amount_at(deal, v, q.ok().as_deref())
}

/// Ревью 08.11: сумма отправки части не больше своей выручки продаж бота на площадке части, если она там есть. Старые
/// сделки (до решения 227) хранят всю выручку одной суммой на площадке закрытия — без потолка «Перегнать» и повтор
/// отправили бы и выручку другой части, и чужие USDT. Продаж бота на площадке нет — сумма как есть. Принимает: ядро,
/// сделку, площадку части, сумму. Возвращает: сумму с потолком. Ошибок нет.
pub fn own_cap(core: &Core, deal: &Deal, v: &VenueId, amount: Decimal) -> Decimal {
    let own = amount_of(core, deal, v);
    if own > Decimal::ZERO {
        amount.min(own)
    } else {
        amount
    }
}

/// Котируемая монета площадки из настроек (None — не задана). Принимает: ядро, площадку. Ошибок нет.
pub(super) fn quote_of(core: &Core, v: &VenueId) -> Option<String> {
    core.inner
        .deps
        .settings
        .text("venue.quote_asset", at(v))
        .ok()
}

/// Область настройки «площадка». Принимает: площадку. Возвращает: At. Ошибок нет.
fn at(v: &VenueId) -> At<'_> {
    At {
        venue: Some(&v.0),
        coin: None,
    }
}
