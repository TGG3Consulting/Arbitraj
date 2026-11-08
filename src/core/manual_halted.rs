//! Файл: core/manual_halted.rs
//! Что делает: ручной разбор сделки, которую бот сам довести не может (решение 100):
//! «Продать сейчас» для остановленной сделки (Halted) — бот продаёт по рынку то, что реально лежит на площадке
//! (не больше количества сделки); «Продано вручную» — человек продал сам и вводит полученные USDT: сделка
//! закрывается этим фактом (вместо предварительного убытка, решение 53). Плюс «сколько монет реально на площадке»
//! (on_hand) — общий для «Продать сейчас» по алерту, по остановленной сделке и «Вернуть боту»; у продаж — on_hand_sell:
//! монеты не на торговом счёте биржи сначала переводятся на него (to_trading.rs, решение 174).
//! «Продать и перегнать» (решение 101) — то же, плюс выручка USDT на площадку покупки (deal.return_usdt); решение 234 —
//! продажа на площадке, где больше после всех затрат перевода (manual_relay.rs), без порогов убытка.
//! Решение 177: продажа кнопкой — ручная команда (risk_count.rs), итог отдельным событием (manual_outcome.rs).
//! Кто вызывает: core/commands.rs (команды SellHalted, SellHaltedReturn, SoldManually), manual.rs (sell_now), manual_return.rs.
//! От чего зависит: deal.rs, ledger.rs, sell.rs, manual.rs, native.rs, to_trading.rs, deps.rs (шлюз — балансы).
//! Настройки: risk.min_bnb_balance — через native.rs (BNB на DEX — сверх резерва на газ, решение 62).

use std::str::FromStr;

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};

use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Сколько монет можно продать на площадке: реальный свободный баланс (BNB на DEX — сверх резерва на газ и газа
/// продажи), не больше `cap`. Решение 132: у DEX — баланс кошелька сделки. Принимает: ядро, сделку, площадку, монету,
/// предел. Возвращает: количество и заметку для хронологии. Ошибки: шлюз, балансы, ноль, нет кошелька сделки.
pub async fn on_hand(
    core: &Core,
    deal: &Deal,
    venue: &VenueId,
    asset: &Asset,
    cap: Decimal,
) -> Result<(Decimal, Option<String>), String> {
    let gw = super::dex_account::venue_of(core, deal, venue).await?;
    let on = super::venue_ticker::ticker_on(core, asset, venue);
    let free = free_of(core, gw.as_ref(), venue, asset).await?;
    let native = super::native::is_native(core, asset);
    let free = super::native::spendable(core, venue, native, &on, free)?;
    let (free, note) = super::native::minus_sell_gas(core, &gw, venue, asset, free).await;
    if free <= Decimal::ZERO {
        return Err(format!(
            "на {} нет свободных {} — если продали сами, нажмите «Продано вручную»",
            venue.0, asset.ticker
        ));
    }
    Ok((free.min(cap), note))
}

/// Свободный баланс монеты на площадке (торговый счёт биржи; у DEX — кошелёк вида `gw`, решение 132), без резервов.
/// Общий для on_hand и сверки остатка перед автоматической продажей (core/remain_fact.rs, решение 222).
/// Принимает: ядро, адаптер (вид на счёт сделки), площадку, монету. Возвращает: free (монеты нет — 0). Ошибки: шлюз.
pub async fn free_of(
    core: &Core,
    gw: &dyn crate::gateway::contract::Venue,
    venue: &VenueId,
    asset: &Asset,
) -> Result<Decimal, String> {
    // Решение 113: баланс одной монеты (у DEX — 1–2 запроса к узлу, а не по каждому токену кошелька).
    // Решение 211: баланс площадки — у биржи по её тикеру, у DEX по адресу (balance_key); резерв BNB — по тикеру.
    Ok(gw
        .balance(&super::venue_ticker::balance_key(core, asset, venue))
        .await
        .map_err(|e| format!("{}: балансы: {}", venue.0, e.message))?
        .map_or(Decimal::ZERO, |x| x.free))
}

/// «Продать сейчас» / «Продать и перегнать» (решение 174, живой случай ION 05.11): монеты сделки, лежащие на бирже не
/// на торговом счёте (фонд / main / Funding), сначала переводятся на торговый общим шагом to_trading::top_up (тот же, что у
/// кнопки «Продать» панели «Балансы»), не больше количества сделки; затем — on_hand по торговому счёту.
/// Принимает и возвращает — как on_hand (заметка — с текстом перевода). Ошибки: как on_hand и сбой перевода.
pub async fn on_hand_sell(
    core: &Core,
    deal: &Deal,
    venue: &VenueId,
    asset: &Asset,
    cap: Decimal,
) -> Result<(Decimal, Option<String>), String> {
    let gw = super::dex_account::venue_of(core, deal, venue).await?;
    // Решение 211: площадке — её тикер монеты.
    let on = super::venue_ticker::on_venue(core, asset, venue);
    let moved = super::to_trading::top_up(gw.as_ref(), &on, cap).await?;
    match (on_hand(core, deal, venue, asset, cap).await, moved) {
        (Ok((q, note)), Some(m)) => Ok((
            q,
            Some(match note {
                Some(n) => format!("{m}; {n}"),
                None => m,
            }),
        )),
        (Ok(x), None) => Ok(x),
        (Err(e), Some(m)) => Err(format!("{m}; {e}")),
        (Err(e), None) => Err(e),
    }
}

/// Ставит количество сделки по реальному балансу (заметка, если меньше). Ошибок нет.
pub fn set_qty(deal: &mut Deal, qty: Decimal, note: Option<String>) {
    if qty < deal.qty {
        deal.note(format!(
            "На площадке {qty} из {} — продаётся то, что есть",
            deal.qty
        ));
    }
    deal.qty = qty;
    if let Some(n) = note {
        deal.note(n);
    }
}

/// «Продать сейчас» для остановленной сделки; ret — «Продать и перегнать» (решение 101). Возвращает: текст. Ошибки: сделки нет, не остановлена, монет нет.
pub async fn sell_halted(core: &Core, deal_id: &str, ret: bool) -> Result<String, String> {
    // Проверка и смена шага под одним замком — двойное нажатие не запустит две продажи.
    let mut deal = {
        let mut deals = lk(&core.inner.deals);
        let d = deals
            .get_mut(deal_id)
            .ok_or(format!("сделки {deal_id} нет"))?;
        if d.step != Step::Halted {
            return Err(format!(
                "сделка {deal_id} не остановлена (шаг {:?})",
                d.step
            ));
        }
        if d.qty <= Decimal::ZERO {
            return Err(format!(
                "в сделке {deal_id} монет нет — закройте её «Продано вручную»"
            ));
        }
        d.step = Step::Selling;
        d.clone()
    };
    // Решение 118 (Я-2): одна сделка на монету — пока по монете идёт другая, продажа не запускается.
    let key = super::coin_key::CoinKey::of(&deal.direction.asset);
    if !lk(&core.inner.active).insert(key.clone()) {
        if let Some(d) = lk(&core.inner.deals).get_mut(deal_id) {
            d.step = Step::Halted;
        }
        return Err(format!(
            "по монете {} идёт другая сделка — продайте после её окончания",
            deal.direction.asset.ticker
        ));
    }
    match on_hand_sell(core, &deal, &deal.at, &deal.direction.asset, deal.qty).await {
        Ok((qty, note)) => set_qty(&mut deal, qty, note),
        Err(e) => {
            deal.step = Step::Halted;
            deal.note(format!("«Продать сейчас»: {e}"));
            super::ledger::save(core, &deal).await;
            lk(&core.inner.active).remove(&key);
            return Err(e);
        }
    }
    if ret {
        // Решения 101 и 234: лучшая площадка после затрат перевода (manual_relay.rs), затем выручка USDT — на площадку
        // покупки (usdt_return.rs).
        deal.return_usdt = true;
        deal.note("«Продать и перегнать»: оценка всех площадок, продажа там, где больше после затрат, затем выручка на площадку покупки");
    } else {
        deal.note("«Продать сейчас»: продажа по рынку без порогов убытка (решение 234)");
    }
    super::ledger::save(core, &deal).await;
    let text = if ret {
        format!(
            "Сделка {deal_id}: оценка площадок для {} {} (сейчас на {}) и продажа по рынку запущены — итог придёт отдельно",
            deal.qty, deal.direction.asset.ticker, deal.at.0
        )
    } else {
        format!(
            "Сделка {deal_id}: продажа {} {} на {} по рынку запущена — итог придёт отдельно",
            deal.qty, deal.direction.asset.ticker, deal.at.0
        )
    };
    let c = core.clone();
    // Решение 177: ручная команда — не «ошибка площадки подряд» (risk_count.rs); итог — manual_outcome.rs.
    // Решение 234: кнопка — без порогов убытка на всём пути (manual_sell.rs); «Продать и перегнать» — manual_relay.rs.
    tokio::spawn(super::risk_count::manual(async move {
        if ret {
            super::manual_relay::run(&c, deal).await;
        } else {
            super::sell::market(&c, deal, super::sell::BUTTON).await;
        }
        lk(&c.inner.active).remove(&key);
    }));
    Ok(text)
}

/// «Продано вручную»: человек продал монеты сам и вводит полученные USDT. Сделка (остановленная, ждущая решения
/// или «Беру на себя») закрывается фактом; её алерт и запись «Беру на себя» снимаются.
/// Принимает: id сделки, выручку USDT (строка, ≥ 0), кто. Возвращает: текст. Ошибки: сумма, сделки нет, шаг.
pub async fn sold_manually(
    core: &Core,
    deal_id: &str,
    usdt: &str,
    who: &str,
) -> Result<String, String> {
    let got = Decimal::from_str(usdt.trim().replace(',', ".").as_str())
        .map_err(|_| format!("выручка «{usdt}» — не число"))?;
    if got < Decimal::ZERO {
        return Err("выручка не может быть отрицательной".into());
    }
    let mut deal = {
        let mut deals = lk(&core.inner.deals);
        let d = deals
            .get_mut(deal_id)
            .ok_or(format!("сделки {deal_id} нет"))?;
        if !matches!(d.step, Step::Halted | Step::TakenOver | Step::WaitingHuman) {
            return Err(format!(
                "сделку {deal_id} ведёт бот или она закрыта (шаг {:?})",
                d.step
            ));
        }
        // Метка «идёт разбор» — повторное нажатие получит отказ.
        d.step = Step::Selling;
        d.clone()
    };
    // Решение 118 (Я-2): одна сделка на монету — пока по монете идёт другая, продажа не запускается.
    let key = super::coin_key::CoinKey::of(&deal.direction.asset);
    if !lk(&core.inner.active).insert(key.clone()) {
        if let Some(d) = lk(&core.inner.deals).get_mut(deal_id) {
            d.step = Step::Halted;
        }
        return Err(format!(
            "по монете {} идёт другая сделка — продайте после её окончания",
            deal.direction.asset.ticker
        ));
    }
    lk(&core.inner.alerts).retain(|_, (_, d)| d.id != deal_id);
    lk(&core.inner.holdings).retain(|_, (_, d)| d.id != deal_id);
    super::manual::persist(core).await;
    let ticker = deal.direction.asset.ticker.clone();
    deal.proceeds += got;
    deal.note(format!(
        "Продано вручную ({who}): {} {ticker}, выручка {got} USDT — ввод человека; газ и комиссия ручной продажи в итог не входят",
        deal.qty
    ));
    deal.qty = Decimal::ZERO;
    // Решение 138: продал человек — бот его выручку не двигает (решение 100); не на площадке покупки — страховка.
    // Решение 224: выручку продаж бота в этой же сделке бот перегоняет сам (usdt_return_deal.rs, шаг prepare).
    deal.usdt_return_skip =
        Some("продано вручную человеком — бот выручку не перегоняет (решение 100)".into());
    super::ledger::finish(core, &mut deal, Step::Closed, "продано вручную человеком").await;
    // Решение 165 (05.11): снять метку монеты — иначе следующая сделка и «Продано вручную» по этой монете
    // получали отказ «идёт другая сделка» до перезапуска (найдено на двух LUCIC Gate).
    lk(&core.inner.active).remove(&key);
    Ok(format!(
        "Сделка {deal_id} закрыта: выручка {got} USDT, итог {} USDT",
        deal.result_usd.unwrap_or_default().round_dp(4)
    ))
}

impl Core {
    /// Остановленные сделки, которые ждут разбора человеком (экран «Ручное управление»): монеты ещё в сделке
    /// или в итоге дня стоит предварительный убыток. Ошибок нет.
    pub fn halted(&self) -> Vec<Deal> {
        let mut v: Vec<Deal> = lk(&self.inner.deals)
            .values()
            .filter(|d| {
                d.step == Step::Halted && (d.qty > Decimal::ZERO || d.provisional_usd.is_some())
            })
            .cloned()
            .collect();
        v.sort_by_key(|d| std::cmp::Reverse(d.updated_at));
        v
    }
}
