//! Файл: venues/bsc_dex/pools/ladder_counter.rs
//! Что делает: суммы «лесенки цен пула» (dex.pmm_ladder_usd; WooFi, Metric OMM, iZiSwap, Biswap V3) в единицах
//! встречного токена пары. Пара с USDT площадки — как раньше (решение 297): сумма ступени в USD = сумма в USDT.
//! Решение 306 (Тигран 15.11.2026: «все включать в такой расчёт»): при dex.pmm_ladder_counter = да пара со встречным
//! токеном X ≠ USDT (WBNB, USDC, FDUSD … — промежуточные монеты, решение 52) тоже получает лесенку: ступень в USDT
//! переводится в количество X тем же механизмом, что пересчёт сумм пробы и глубины (quote/rate.rs best_in: точный вход
//! USDT в лучший достоверный пул «X — USDT» реестра, локальная математика пула — данные своего узла). Пулы «X — USDT»,
//! которые спят (asleep — мельче dex.min_pool_liquidity_usd, обмены не слушаются, quote/select.rs их не котирует), в
//! курс не идут. Нет курса — лесенки нет, пара без котировки («не торгуем вслепую»); смена «курс есть / нет» — запись
//! в журнал (один раз на смену, по паре «DEX, X»). Ступень, на которую глубины пулов «X — USDT» не хватает, — край
//! лесенки (точек после неё нет, как откат квотера). Точки лесенки — ответы квотера САМОГО пула на этих количествах X:
//! курс выбирает только, где взять точки; котировка шага в X интерполируется по ним без пересчёта через USD.
//! Кто вызывает: pools/load_woo_ladder.rs (read), pools/load_metric_ladder.rs (read_with), pools/per_block.rs (on_head).
//! От чего зависит: ctx.rs, tokens.rs, registry.rs, discover.rs, quote/rate.rs (best_in), load_woo_ladder.rs (usd,
//! usdt_quote).
//! Настройки: dex.pmm_ladder_usd, dex.pmm_ladder_counter [DEX] (не задана / «нет» — пары не с USDT без лесенки, 1 в 1
//! как до решения 306).

use std::collections::HashMap;
use std::sync::Mutex;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::quote::rate;
use super::super::tokens::Tokens;
use super::discover::Found;
use super::load_woo_ladder::{usd, usdt_quote};
use super::registry::{Entry, Registry};

/// Имя настройки решения 306.
pub const SETTING: &str = "dex.pmm_ladder_counter";

/// (id DEX, встречный токен) → последний записанный в журнал итог «курс есть» (за время работы процесса).
static TOLD: Mutex<Option<HashMap<(String, Address), bool>>> = Mutex::new(None);

/// Включена ли лесенка пар не с USDT у DEX. Принимает: окружение, id DEX. Возвращает: true — dex.pmm_ladder_counter =
/// да; не задана, «нет», ошибка чтения — false. Ошибок нет.
pub fn enabled(ctx: &Ctx, id: &str) -> bool {
    matches!(ctx.dex_flag(SETTING, id), Ok(true))
}

/// Суммы ступеней лесенки в единицах встречного токена пары. Принимает: окружение, токены, реестр (курс X → USDT),
/// id DEX, встречный токен пары. Возвращает: суммы по возрастанию; пусто — лесенки нет (dex.pmm_ladder_usd не задана;
/// USDT площадки не задан; встречный не USDT и dex.pmm_ladder_counter не «да»; нет курса). Ошибки: неверная
/// dex.pmm_ladder_usd (NotConfigured), decimals с узла.
pub async fn amounts(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    id: &str,
    quote: Address,
) -> GatewayResult<Vec<U256>> {
    let ladder = usd(ctx, id)?;
    if ladder.is_empty() {
        return Ok(Vec::new());
    }
    let usdt = tokens.usdt(ctx).ok();
    if usdt_quote(usdt, quote) {
        // Решение 297: пара с USDT — 1 в 1 как раньше.
        let dec = tokens.decimals(ctx, quote).await?;
        return Ok(scaled(&ladder, dec));
    }
    let Some(usdt) = usdt else {
        return Ok(Vec::new());
    };
    if !enabled(ctx, id) {
        return Ok(Vec::new());
    }
    let dec = tokens.decimals(ctx, usdt).await?;
    let pools: Vec<Entry> = reg
        .by_pair(quote, usdt)
        .into_iter()
        .filter(|e| !e.asleep)
        .collect();
    let got = convert(ctx, &pools, quote, usdt, &scaled(&ladder, dec));
    tell(ctx, id, quote, !got.is_empty()).await;
    Ok(got)
}

/// Суммы USDT → количества встречного токена (без сети; её же зовут тесты). Принимает: окружение (комиссия V2 у
/// pool_quote), пулы «X — USDT», X, USDT, суммы USDT по возрастанию. Возвращает: количества X по возрастанию — до
/// первой ступени, которую не посчитать (нет достоверного пула, глубины не хватает, ноль или не больше предыдущей).
/// Ошибок нет.
pub fn convert(
    ctx: &Ctx,
    pools: &[Entry],
    quote: Address,
    usdt: Address,
    usdt_amounts: &[U256],
) -> Vec<U256> {
    let mut out: Vec<U256> = Vec::new();
    for a in usdt_amounts {
        match rate::best_in(ctx, pools, quote, usdt, *a) {
            Ok(x) if !x.is_zero() && out.last().is_none_or(|l| *l < x) => out.push(x),
            _ => break,
        }
    }
    out
}

/// Суммы в единицах токена с `dec` знаками (усечение). Принимает: суммы, decimals. Возвращает: список (не влезло в
/// u128 — сумма пропущена, как раньше в load_woo_ladder.rs read). Ошибок нет.
pub fn scaled(ladder: &[Decimal], dec: u8) -> Vec<U256> {
    let Some(pow) = 10u64.checked_pow(u32::from(dec)) else {
        return Vec::new();
    };
    let unit = Decimal::from(pow);
    ladder
        .iter()
        .filter_map(|v| v.checked_mul(unit))
        .filter_map(|v| v.trunc().to_u128().map(U256::from))
        .collect()
}

/// Перечитывать ли пул iZiSwap / Biswap V3 БЕЗ USDT в паре на каждом блоке (решение 306). Ревью t118 их исключил,
/// потому что лесенки у них не было; при dex.pmm_ladder_counter = да (и заданной dex.pmm_ladder_usd) она есть — у
/// Biswap V3 комиссия голосуемая без события пула, поэтому такие пулы перечитываются каждым блоком, как пулы с USDT.
/// Metric и WooFi перечитываются каждым блоком и без этого (per_block.rs needs). Принимает: окружение, пул, USDT
/// площадки. Ошибок нет.
pub fn each_block(ctx: &Ctx, f: &Found, usdt: Option<Address>) -> bool {
    let id = f.family.id;
    f.family.proto.is_izi()
        && usdt.is_some_and(|u| f.token0 != u && f.token1 != u)
        && enabled(ctx, id)
        && usd(ctx, id).is_ok_and(|l| !l.is_empty())
}

/// Запись в журнал при смене «курс X → USDT есть / нет» у DEX (первый итог «нет» — тоже). Ошибок нет.
async fn tell(ctx: &Ctx, id: &str, quote: Address, have: bool) {
    let before = TOLD
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert((id.to_string(), quote), have);
    if before == Some(have) || (before.is_none() && have) {
        return;
    }
    let (level, text) = if have {
        (
            Level::Info,
            format!(
                "Лесенка цен пула {id}: курс {quote:#x} → USDT снова есть — пары с этим встречным токеном получают \
                 лесенку в его количестве ({SETTING}, решение 306)"
            ),
        )
    } else {
        (
            Level::Warn,
            format!(
                "Лесенка цен пула {id}: нет курса {quote:#x} → USDT (в реестре нет достоверного неспящего пула \
                 «{quote:#x} — USDT» или его глубины не хватает на первую ступень dex.pmm_ladder_usd) — лесенка пар с \
                 этим встречным токеном не строится, пары без котировки ({SETTING}, решение 306)"
            ),
        )
    };
    ctx.log(level, "dex.pool", text, None).await;
}
