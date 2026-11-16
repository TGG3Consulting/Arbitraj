//! Файл: venues/bsc_dex/pools/off_map_bins.rs
//! Что делает: решение 332 (задача t154) — «цена ушла за прочитанную часть пула → пул сразу перечитывается» у пулов из
//! бинов: PancakeSwap Infinity Bin (PoolState::Bin) и Liquidity Book V2.1 (PoolState::LbBin). Бот держит в памяти не
//! все бины, а полосу вокруг активного (pools/load_bin.rs, pools/load_lb_bins.rs): каждая сторона читается, пока
//! стоимость прочитанных бинов не достигнет dex.bin_depth_usd (или бины не кончатся). Бин вне полосы молча не
//! записывается (bins.rs set), расчёт обмена у края полосы останавливается с неполным итогом (math/bin_swap.rs,
//! math/lb_swap.rs), глубина считается до края полосы (quote/depth_band.rs) — после сильного движения цены пул выглядит
//! «мелким» до Refresh или переподключения (до решения 332 — ничем не перечитывался: обмен перечитывает только
//! затронутые бины, per_block_lb.rs — только активный бин и комиссию).
//! Правило: после применённого события бинов (pools/listen_log.rs, ветка Applied::Bins — Swap, Mint/Burn, Donate у
//! Infinity Bin; Swap, Deposited/Withdrawn, CompositionFees, FlashLoan у LB) считается прочитанная стоимость каждой
//! стороны от НОВОГО активного бина — тем же usdt_value и тем же правилом, что у загрузчика (вниз — бины полосы ниже
//! активного, вверх — выше). Сторона, прочитанная до конца пула (край полосы 0 внизу / 0xFFFFFF вверху — так загрузчик
//! отмечает «дальше бинов нет»), не проверяется. Если стоимость хотя бы одной стороны меньше
//! dex.bin_reread_left_pct процентов от dex.bin_depth_usd — пул перечитывается целиком на блоке события
//! (pools/refresh_event.rs refresh — тот же путь и тот же класс dex.event_refresh_urgency, что у V3 off_map).
//! Активный бин за краем полосы — стоимость этой стороны 0, перечитывание.
//! Защита от перечитывания на каждом событии: сразу после чтения каждая сторона по построению не меньше
//! dex.bin_depth_usd (или прочитана до конца) — повтора нет, пока обмены или выводы не съедят больше
//! (100 − dex.bin_reread_left_pct)% прочитанного в эту сторону; после полного перечитывания на блоке остальные события
//! этого блока уже учтены (registry.rs has_applied) — не чаще раза на блок на пул.
//! Не применяется (Keep — как до решения 332): настройка не задана у этой DEX; dex.bin_depth_usd не задана; у пары LB
//! нет встречного токена записи (загрузчик тогда читает все бины — полоса до конца пула). Не считается (Unknown — не
//! перечитывается, Warn): decimals встречного токена ещё не известны; стоимость бина не считается (usdt_value).
//! Ревью 332: (Н2) проверка не считается (decimals встречного токена неизвестны, стоимость бина не считается) — не молча:
//! Warn dex.pool; (Н7) Info о перечитывании — обе не чаще раза в log.bin_reread_interval_s на пул
//! (off_map_bins_log.rs; не задана — этих записей нет). (Н6) края «прочитано до конца» — bins.rs (down_to_end / up_to_end).
//! (Б1) Liquidity Book каждым блоком (per_block_lb.rs) активный бин больше не двигает (lb_head.rs) — активный бин
//! меняют только события, после которых и идёт эта проверка.
//! Кто вызывает: pools/listen_log.rs (после перечитывания бинов события — after).
//! От чего зависит: ctx.rs, bins.rs, load_bin.rs (Depth, usdt_value — та же оценка стоимости, что у загрузчика),
//! refresh_event.rs (refresh, Event), registry.rs, listen.rs (Pools), group.rs (Msg), tokens.rs (decimals_known),
//! off_map_bins_log.rs (частота записей).
//! Настройки: dex.bin_reread_left_pct [DEX] (необязательная, % больше 0 и меньше 100; не задана — 1 в 1 как раньше),
//! dex.bin_depth_usd [bsc_dex], log.bin_reread_interval_s (вся система, необязательная — только записи журнала).
//! Края полосы и getNextNonEmptyBin: IBinPoolManager.sol L161-L167 и BinPool.sol L293-L297 (pancakeswap/infinity-core
//! 3c5ef1be), TreeMath.sol L102-L202 (lfj-gg/joe-v2@v2.1.0) — нет бина ниже → type(uint24).max, выше → 0 (bins.rs).

use rust_decimal::Decimal;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::bins::BinMap;
use super::group::Msg;
use super::listen::Pools;
use super::load_bin::{Depth, usdt_value};
use super::off_map_bins_log::{self as rate, What};
use super::refresh_event::{self, Event};
use super::registry::Entry;

/// Настройка решения 332 (область «DEX»).
pub const SETTING: &str = "dex.bin_reread_left_pct";

/// Итог проверки (verdict).
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Не перечитывать: прочитано достаточно, вид не из бинов, настройка не задана, у пары LB нет встречного токена.
    Keep,
    /// Перечитать пул целиком.
    Reread,
    /// Настройка задана, но проверка не считается (причина) — пул не перечитывается; Warn (ревью 332, Н2).
    Unknown(String),
}

/// Прочитанная стоимость сторон от активного бина. None — сторона прочитана до конца пула (не проверяется).
#[derive(Debug, Clone, PartialEq)]
pub struct Sides {
    /// Бины полосы ниже активного.
    pub down: Option<Decimal>,
    /// Бины полосы выше активного.
    pub up: Option<Decimal>,
}

/// Стоимость сторон (см. шапку). Принимает: окружение (для текста ошибки), бины записи, активный бин, оценку стоимости
/// (как у загрузчика). Возвращает: Sides. Ошибки: BadResponse — цена бина не считается (как у usdt_value).
pub fn sides(ctx: &Ctx, bins: &BinMap, active: u32, d: &Depth) -> GatewayResult<Sides> {
    let down = if bins.down_to_end() {
        None
    } else {
        let hi = active.checked_sub(1);
        Some(match hi {
            Some(hi) => value(ctx, bins, bins.range.0, hi, d)?,
            None => Decimal::ZERO,
        })
    };
    let up = if bins.up_to_end() {
        None
    } else {
        Some(value(ctx, bins, active.saturating_add(1), bins.range.1, d)?)
    };
    Ok(Sides { down, up })
}

/// Сумма стоимости непустых бинов с id в [lo; hi] (пусто, если lo > hi). Ошибки: как у usdt_value.
fn value(ctx: &Ctx, bins: &BinMap, lo: u32, hi: u32, d: &Depth) -> GatewayResult<Decimal> {
    if lo > hi {
        return Ok(Decimal::ZERO);
    }
    let mut sum = Decimal::ZERO;
    for (id, (x, y)) in bins.reserves.range(lo..=hi) {
        sum += usdt_value(ctx, *x, *y, *id, bins.step, d)?;
    }
    Ok(sum)
}

/// Мало ли прочитано хотя бы в одну сторону: стоимость меньше `pct` процентов от `limit`. Ошибок нет.
pub fn short(s: &Sides, limit: Decimal, pct: Decimal) -> bool {
    let floor = limit * pct / Decimal::ONE_HUNDRED;
    s.down.is_some_and(|v| v < floor) || s.up.is_some_and(|v| v < floor)
}

/// Нужно ли перечитать пул (см. шапку). Принимает: окружение, запись, decimals встречного токена записи (Entry.usdt;
/// None — ещё не известны). Возвращает: true — перечитывать; вид не из бинов, нет бинов, настройка не задана, у пары LB
/// нет USDT, стоимость не считается — false. Ошибок нет.
pub fn check(ctx: &Ctx, e: &Entry, usdt_decimals: Option<u8>) -> bool {
    verdict(ctx, e, usdt_decimals) == Verdict::Reread
}

/// Итог проверки с причиной «не считается» (см. Verdict). Принимает: как check. Ошибок нет.
pub fn verdict(ctx: &Ctx, e: &Entry, usdt_decimals: Option<u8>) -> Verdict {
    let Some(bins) = e.bins.as_ref() else {
        return Verdict::Keep;
    };
    let (active, usdt_is_y) = match &e.pool.state {
        // Infinity Bin — как load.rs (ветка PoolKind::Bin): USDT — Y, если он token1 записи.
        PoolState::Bin { active_id, .. } => (*active_id, e.usdt == e.pool.token1),
        // LB — как load_lb.rs: X/Y по порядку создателя пула; без USDT в паре полоса — до конца пула.
        PoolState::LbBin(d) => {
            let (x, y) = if d.x_is_token0 {
                (e.pool.token0, e.pool.token1)
            } else {
                (e.pool.token1, e.pool.token0)
            };
            if e.usdt != x && e.usdt != y {
                return Verdict::Keep;
            }
            (d.active_id, e.usdt == y)
        }
        _ => return Verdict::Keep,
    };
    let Ok(pct) = ctx.dex_dec(SETTING, e.family.id) else {
        return Verdict::Keep;
    };
    // dex.bin_depth_usd не задана — загрузчик пулов из бинов тоже не работает (load_bin.rs, load_lb_bins.rs): не считаем.
    let Ok(limit) = ctx.dec("dex.bin_depth_usd") else {
        return Verdict::Keep;
    };
    let Some(usdt_decimals) = usdt_decimals else {
        return Verdict::Unknown(format!(
            "decimals встречного токена {:#x} ещё не известны",
            e.usdt
        ));
    };
    let d = Depth {
        usdt_is_y,
        usdt_decimals,
    };
    match sides(ctx, bins, active, &d) {
        Ok(s) if short(&s, limit, pct) => Verdict::Reread,
        Ok(_) => Verdict::Keep,
        Err(err) => Verdict::Unknown(err.message),
    }
}

/// После применённого события бинов (pools/listen_log.rs): прочитанного в какую-то сторону мало — пул перечитывается
/// целиком на блоке события, запись — в реестр. Принимает: окружение пачки, ссылку, блок события, канал сессии.
/// Возвращает: Ok — проверено (перечитан или не нужно). Ошибки: как у load::refresh (их разбирает listen_log.rs).
pub async fn after(
    p: &Pools,
    r: &PoolRef,
    block: u64,
    queued: &UnboundedReceiver<Msg>,
) -> GatewayResult<()> {
    let Some(e) = p.reg.get(r) else {
        return Ok(());
    };
    match verdict(&p.ctx, &e, p.tokens.decimals_known(e.usdt)) {
        Verdict::Keep => return Ok(()),
        Verdict::Unknown(why) => {
            // Ревью 332 (Н2): не молча — пул может остаться «мелким»; не чаще log.bin_reread_interval_s на пул.
            if rate::due(&p.ctx, r, What::Unknown) {
                let msg = format!(
                    "{} {}: проверка {SETTING} не считается ({why}) — пул не перечитан и может остаться мелким \
                     (расчёт обмена и глубина — только до края прочитанной полосы бинов)",
                    e.family.title, e.ticker
                );
                p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
            }
            return Ok(());
        }
        Verdict::Reread => {}
    }
    // Ревью 332 (Н7): запись о перечитывании — не чаще log.bin_reread_interval_s на пул.
    if rate::due(&p.ctx, r, What::Reread) {
        let range = e.bins.as_ref().map(|b| b.range).unwrap_or_default();
        let msg = format!(
            "{} {}: в сторону цены прочитано бинов меньше {SETTING} от dex.bin_depth_usd (прочитаны бины {}…{}) — \
             пул перечитывается вокруг нового активного бина",
            e.family.title, e.ticker, range.0, range.1
        );
        p.ctx.log(Level::Info, "dex.pool", msg, None).await;
    }
    let x =
        refresh_event::refresh(p, &e, block, &Event::new("off_map_bins", block, queued)).await?;
    p.reg.put(x);
    Ok(())
}
