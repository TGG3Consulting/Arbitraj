//! Расчётная монета Base. Модель конверсии на бирже использует объём на уровнях и комиссию этой пары.
//! Источники: https://developers.circle.com/stablecoins/usdc-contract-addresses и справочники сетей самих бирж.
//! Котировки и реальный итог исполнения имеют отдельные типы. Денежные операции здесь не выполняются.

use alloy_primitives::{Address, address};
use rust_decimal::Decimal;
use serde::Serialize;

use super::engine::{Core, rd};
use super::types::{CoinNetworkInfo, CostKind, Side, VenueId};
use crate::gateway::events::SpotPairState;
use crate::gateway::frozen::{QuoteSnapshot, deepest};
use crate::settings::read::At;

/// Адрес нативного USDC у эмитента Circle, chain ID 8453. USDbC имеет другой адрес.
pub const BASE_USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

/// Проверка единицы исторического пути, который ведёт учёт и переводит только USDT.
pub fn require_usdt(a: &str, b: &str) -> Result<(), String> {
    if a == "USDT" && b == "USDT" {
        Ok(())
    } else {
        Err(format!(
            "Расчёт {a} → {b} на Base требует отдельной конверсии на исходной бирже; путь прямого возврата USDT неприменим"
        ))
    }
}

/// Отбирает запись именно этой биржи, сети и контракта. Для вывода и ввода действуют разные флаги.
pub fn usdc_network(core: &Core, cex: &VenueId, withdraw: bool) -> Result<CoinNetworkInfo, String> {
    let settings = &core.inner.deps.settings;
    let at = At {
        venue: Some(&cex.0),
        coin: None,
    };
    let network = settings
        .text("venue.base_network", at)
        .map_err(|e| e.message)?;
    let unsupported = || {
        format!(
            "{}: нет подтверждённого нативного Base USDC в сети {network}",
            cex.0
        )
    };
    let networks = rd(&core.inner.networks);
    let mut rows = networks
        .get(cex)
        .into_iter()
        .flat_map(|all| all.iter())
        .filter(|r| {
            r.network == network
                && r.venue == *cex
                && r.asset.ticker == "USDC"
                && r.asset.chain.as_ref().is_some_and(|c| c.as_str() == "base")
                && r.asset.contract == Some(BASE_USDC)
        });
    let row = rows.next().ok_or_else(unsupported)?;
    if rows.next().is_some() {
        return Err(format!("{}: неоднозначная запись Base USDC", cex.0));
    }
    super::dirctx_venue::fresh(core, row)?;
    if withdraw && !row.withdraw_enabled || !withdraw && !row.deposit_enabled {
        return Err(format!(
            "{}: {} Base USDC закрыт",
            cex.0,
            if withdraw {
                "вывод"
            } else {
                "депозит"
            }
        ));
    }
    Ok(row.clone())
}

/// Модель одной конверсии на CEX. Обе валюты именованы: размеры и комиссии сохраняются в своей валюте.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conversion {
    pub venue: VenueId,
    pub side: Side,
    pub order_usdc: Decimal,
    pub net_usdc: Decimal,
    pub net_usdt: Decimal,
    pub fee_usdc: Decimal,
    pub fee_usdt: Decimal,
    pub limit_price: Decimal,
    /// Для покупки — необходимый резерв USDT по лимитной цене; для продажи — нижняя граница выручки.
    pub limit_usdt: Decimal,
}

/// Лимитный ордер IOC/FOK на бирже-источнике. Объём USDC округляется вниз, цена берётся с предельного уровня.
/// Возвращает суммы сторон с комиссией. Нехватка глубины или неизвестная валюта комиссии закрывает план.
pub async fn conversion(
    snap: &QuoteSnapshot,
    rules: &SpotPairState,
    cex: &VenueId,
    side: Side,
    quantity: Decimal,
) -> Result<Conversion, String> {
    let book = snap.book().ok_or("Нет стакана USDC/USDT")?;
    if book.venue != *cex || book.asset.ticker != "USDC" || rules.ticker != "USDC" {
        return Err("Стакан или правила пары не относятся к исходной бирже и USDC".into());
    }
    if !rules.online
        || rules.order_block.is_some()
        || !(if side == Side::Buy {
            rules.api_buy
        } else {
            rules.api_sell
        })
    {
        return Err("Требуемая сторона пары USDC/USDT закрыта для ордеров".into());
    }
    if rules.step_size <= Decimal::ZERO || quantity <= Decimal::ZERO {
        return Err("Для конверсии нужны положительные размер и шаг USDC".into());
    }
    let qty = quantity
        - quantity
            .checked_rem(rules.step_size)
            .ok_or("Невозможно округлить количество USDC")?;
    if qty <= Decimal::ZERO {
        return Err("Количество USDC меньше шага пары".into());
    }
    let limit_price = deepest(book, side, qty)
        .filter(|p| *p > Decimal::ZERO)
        .ok_or("Недостаточно глубины USDC/USDT")?;
    super::costs_quote::order_limits(&cex.0, Some(rules), qty, limit_price)
        .map_err(|e| e.to_string())?;
    let q = snap.quote(qty).await.map_err(|e| e.message)?;
    if q.venue != *cex
        || q.side != side
        || q.asset.ticker != "USDC"
        || q.fillable_qty != qty
        || q.avg_price <= Decimal::ZERO
    {
        return Err("Котировка конверсии не покрывает объём на нужной стороне пары".into());
    }
    if !q.costs.iter().any(|fee| fee.kind == CostKind::TradeFee) {
        return Err("Биржа не сообщила комиссию конверсии USDC/USDT".into());
    }
    let mut fee_usdc = Decimal::ZERO;
    let mut fee_usdt = Decimal::ZERO;
    for fee in q.costs.iter().filter(|c| !c.in_price) {
        if fee.paid_in.is_some() || fee.amount < Decimal::ZERO {
            return Err(
                "Для комиссии отдельной монетой требуется исполнимая оценка и резерв этой монеты"
                    .into(),
            );
        }
        match fee.asset.as_str() {
            "USDC" => fee_usdc += fee.amount,
            "USDT" => fee_usdt += fee.amount,
            ticker => {
                return Err(format!(
                    "Неизвестна стоимость комиссии конверсии в {ticker}"
                ));
            }
        }
    }
    let gross = qty
        .checked_mul(q.avg_price)
        .ok_or("Сумма конверсии вне диапазона")?;
    let (net_usdc, net_usdt) = if side == Side::Buy {
        (qty - fee_usdc, gross + fee_usdt)
    } else {
        (qty + fee_usdc, gross - fee_usdt)
    };
    if net_usdc <= Decimal::ZERO || net_usdt <= Decimal::ZERO {
        return Err("Комиссия превышает результат конверсии".into());
    }
    let limit_gross = qty
        .checked_mul(limit_price)
        .ok_or("Сумма лимитного ордера вне диапазона")?;
    let limit_fee = if fee_usdt.is_zero() {
        Decimal::ZERO
    } else {
        fee_usdt
            .checked_div(gross)
            .and_then(|rate| rate.checked_mul(limit_gross))
            .ok_or("Комиссия лимитного ордера вне диапазона")?
    };
    let limit_usdt = if side == Side::Buy {
        limit_gross + limit_fee
    } else {
        limit_gross - limit_fee
    };
    Ok(Conversion {
        limit_usdt,
        venue: cex.clone(),
        side,
        order_usdc: qty,
        net_usdc,
        net_usdt,
        fee_usdc,
        fee_usdt,
        limit_price,
    })
}

/// Готовит свежую конверсию из адаптера именно выбранной биржи. Публичного процента комиссии пары
/// для исполнения недостаточно: снимок адаптера содержит комиссию данного аккаунта.
pub async fn preview(
    core: &Core,
    cex: &VenueId,
    side: Side,
    quantity: Decimal,
) -> Result<Conversion, String> {
    let (_, rules, book) = terms(core, cex, side).await?;
    conversion(&book, &rules, cex, side, quantity).await
}

/// Книга и торговые условия конверсии берутся один раз для расчёта соседних объёмов.
pub(super) async fn terms(
    core: &Core,
    ce: &VenueId,
    side: Side,
) -> Result<(CoinNetworkInfo, SpotPairState, QuoteSnapshot), String> {
    let at = At {
        venue: Some(&ce.0),
        coin: None,
    };
    if core
        .inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .map_err(|e| e.message)?
        != "USDT"
    {
        return Err("Расчётная валюта исходной биржи должна быть USDT".into());
    }
    let network = usdc_network(core, ce, side == Side::Buy)?;
    let rules = super::dirctx_venue::pair_rules(core, ce, "USDC")?
        .ok_or("Для конверсии требуется спотовая биржа")?;
    let book = super::exec_book::book(core, ce, &network.asset, side)
        .await
        .map_err(|e| e.to_string())?;
    Ok((network, rules, book.snap))
}

/// Максимальный исполнимый ордер продажи в пределах имеющихся USDC и ими же уплачиваемой комиссии.
pub(super) async fn sell_within(
    core: &Core,
    cex: &VenueId,
    available: Decimal,
) -> Result<Conversion, String> {
    let (_, rules, book) = terms(core, cex, Side::Sell).await?;
    let q = conversion(&book, &rules, cex, Side::Sell, available).await?;
    if q.net_usdc <= available {
        return Ok(q);
    }
    let qty = available
        .checked_mul(q.order_usdc)
        .and_then(|v| v.checked_div(q.net_usdc))
        .ok_or("Не удалось вычислить ордер с комиссией USDC")?;
    let q = conversion(&book, &rules, cex, Side::Sell, qty).await?;
    if q.net_usdc > available {
        return Err("Комиссия продажи превышает доступные USDC".into());
    }
    Ok(q)
}

/// Минимальное число шагов ордера с подтверждённым зачислением required на Base. Проверяются
/// фактические комиссии книги, сетевые пределы и обе точности; промежуточные оценки не становятся заявками.
pub(super) async fn fund_for(
    core: &Core,
    cex: &VenueId,
    required: Decimal,
) -> Result<Conversion, String> {
    use rust_decimal::prelude::ToPrimitive;
    let (network, rules, book) = terms(core, cex, Side::Buy).await?;
    let snapshot = book.book().ok_or("Нет книги финансирования USDC")?;
    let level = snapshot
        .asks
        .first()
        .filter(|v| v.price > Decimal::ZERO)
        .ok_or("Нет предложения USDC")?;
    let step = rules.step_size;
    if step <= Decimal::ZERO || required <= Decimal::ZERO {
        return Err("Неположительная потребность в USDC или шаг".into());
    }
    let capacity = snapshot
        .asks
        .iter()
        .try_fold(Decimal::ZERO, |n, l| n.checked_add(l.qty))
        .ok_or("Глубина вне диапазона")?;
    let lot = |n: Decimal| {
        n.checked_div(step)
            .and_then(|v| v.floor().to_u128())
            .ok_or("Количество шагов вне диапазона")
    };
    let maximum = capacity.min(rules.max_qty.unwrap_or(capacity));
    let mut high = lot(maximum)?;
    let mut low = lot(required
        .max(rules.min_qty.unwrap_or_default())
        .max(rules.min_notional.unwrap_or_default() / level.price))?;
    let mut chosen = None;
    while low <= high {
        let mid = low + (high - low) / 2;
        let qty = Decimal::try_from(mid)
            .ok()
            .and_then(|m| m.checked_mul(step))
            .ok_or("Размер ордера вне диапазона")?;
        if qty <= Decimal::ZERO {
            low = mid + 1;
            continue;
        }
        let Some(price) = crate::gateway::frozen::deepest(snapshot, Side::Buy, qty) else {
            return Err("Книга не покрывает заявленную глубину".into());
        };
        if rules.max_notional.is_some_and(|v| qty * price > v)
            || network
                .withdraw_max
                .filter(|v| *v > Decimal::ZERO)
                .is_some_and(|v| qty > v && chosen.is_some())
        {
            if mid == 0 {
                break;
            }
            high = mid - 1;
            continue;
        }
        if rules.min_notional.is_some_and(|v| qty * price < v)
            || rules.min_qty.is_some_and(|v| qty < v)
        {
            low = mid + 1;
            continue;
        }
        let q = conversion(&book, &rules, cex, Side::Buy, qty).await?;
        if network
            .withdraw_max
            .filter(|v| *v > Decimal::ZERO)
            .is_some_and(|v| q.net_usdc > v)
        {
            if mid == 0 {
                break;
            }
            high = mid - 1;
            continue;
        }
        let wire = super::settlement_path::funding_wire(&network, q.net_usdc);
        if wire.is_ok_and(|v| v >= required) {
            chosen = Some(q);
            if mid == 0 {
                break;
            }
            high = mid - 1;
        } else {
            low = mid + 1;
        }
    }
    chosen.ok_or_else(||"Нет объёма USDC/USDT, который после комиссии и округления вывода финансирует покупку на Base".into())
}
