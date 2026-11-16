//! Файл: venues/bsc_dex/pools/depth_book.rs
//! Что делает: глубина пулов по направлениям (решение 73) — в памяти, на текущем состоянии пула, с гистерезисом:
//! - глубина покупки и продажи в полосе dex.depth_band_pct (quote/depth_band.rs) пересчитывается, когда сменилось
//!   состояние пула (решение 301 е2: поколение состояния записи, pools/state_gen.rs; раньше — блок и logIndex) или настройки глубины; иначе — из памяти;
//! - направление «глубокое», если его глубина не ниже dex.min_pool_liquidity_usd; глубокое становится мелким, только
//!   когда глубина упала ниже порога × (1 − dex.depth_hysteresis_pct / 100) — пул не мигает на границе;
//! - смена «глубокий ↔ мелкий» по направлению — запись в журнал, по пулу не чаще dex.depth_log_min_s; смена внутри
//!   окна не теряется, а откладывается до его конца (pools/depth_log.rs); настройка необязательна — не задана: глубина
//!   считается, записей о смене нет;
//! - направление стало глубоким — сигнал «пул готов» подаётся здесь же (on_rise): кто бы первым ни посчитал глубину
//!   на новом состоянии (отбор пула, экран «Монеты», проверки), сигнал не теряется и подаётся один раз;
//! - курс промежуточной монеты (пул с BNB, решение 52) — часть ключа: сменился пул «промежуточная — USDT» —
//!   глубина пересчитывается.
//!
//! Решение 301 (б): неудача расчёта глубины по состоянию пула (quote/depth_band.rs band_units) запоминается по тому же
//! ключу (pools/depth_fail.rs) — пока ключ и состояние пула те же, она не пересчитывается при каждом построении графа;
//! причина отдаётся та же, как раньше (экран «Монеты», причина отказа котировки). calcs — счётчик расчётов глубины
//! (замер, без настроек).
//!
//! Решение 307: кривая пула (band_units) помнится по (поколение состояния, полоса) отдельно от ключа (depth_units.rs):
//! сменился только курс встречного токена (поколения пулов X–USDT) или порог/гистерезис — пересчитывается только перевод
//! в USDT (два to_usdt — своп через лучший пул X–USDT, смысл решения 73 тот же); пулы X–USDT — из подготовки (Prep::rate).
//!
//! Решение 336 (задача t157): карта слотов — RwLock: попадание по ключу — под общим чтением (раньше один Mutex на все
//! пулы и все потоки счёта — каждый допуск пула каждой котировки шёл через него по одному); итог тот же.
//!
//! Решение 217: seeded — книга замороженного реестра (registry_frozen.rs); прошлый вердикт гистерезиса — копия на
//! момент снимка (verdicts).
//! Кто вызывает: quote/select.rs (deep_side), pools/listen_log.rs (emit — пересчёт на каждой смене состояния),
//! pools/depth_facts.rs (экран «Монеты»), pools/graph.rs (build_measured — depth_prep_r, решение 301 д), pools/graph_eval.rs
//! (фоновый граф, решение 308).
//! От чего зависит: quote/depth_band.rs, depth_key.rs, depth_fail.rs, depth_units.rs, depth_log.rs, registry.rs, tokens.rs, ctx.rs.
//! Настройки: dex.depth_band_pct, dex.depth_hysteresis_pct, dex.depth_log_min_s, dex.min_pool_liquidity_usd [bsc_dex].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::quote::depth_band::{self, Band};
use super::super::tokens::Tokens;
use super::depth_fail::{Fails, UnitsKey};
use super::depth_key::Prep;
use super::depth_log::{self, Said};
use super::depth_units::Units;
use super::registry::{Entry, Registry};

/// Глубина пула по направлениям и вердикт.
#[derive(Debug, Clone, PartialEq)]
pub struct Depth {
    pub band: Band,
    /// Полоса, % — на момент расчёта.
    pub band_pct: Decimal,
    /// Порог dex.min_pool_liquidity_usd — на момент расчёта.
    pub min: Decimal,
    /// Гистерезис, % — на момент расчёта.
    pub hyst: Decimal,
    pub deep_buy: bool,
    pub deep_sell: bool,
}

pub(super) struct Slot {
    pub(super) depth: Depth,
    key: super::depth_key::Key,
    /// Решение 307: band_units пула и на чём посчитаны (поколение состояния, полоса) — при смене только курса
    /// встречного токена кривая не пересчитывается (depth_units.rs).
    units: Option<(UnitsKey, Units)>,
    /// Журнал смен (depth_log.rs).
    pub(super) said: Said,
    /// Для текстов журнала: вид, тикер, адрес пула.
    pub(super) label: String,
}

/// Глубина всех пулов площадки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Book {
    /// Решение 336 (задача t157): RwLock — попадание по ключу (глубина из памяти) идёт под общим чтением, без очереди
    /// всех потоков счёта на одном замке; пересчёт, журнал смен и сброс — под записью (lock).
    pub(super) m: Arc<RwLock<HashMap<PoolRef, Slot>>>,
    wake: Arc<Mutex<Vec<Weak<Notify>>>>,
    /// Решение 217: книга замороженного реестра — прошлый вердикт гистерезиса, скопированный из живой книги в момент
    /// снимка (verdicts), без журнала и сигналов.
    seed: Option<Arc<Verdicts>>,
    /// Решение 301 (б): неудачи расчёта глубины по ключу (depth_fail.rs; решение 307 — по UnitsKey).
    pub(super) fails: Fails,
    /// Решение 301 (б): сколько раз считалась глубина по состоянию пула (band_units) — замер.
    pub(super) calcs: Arc<AtomicU64>,
}

/// Решение 217: прошлые вердикты гистерезиса пулов снимка (глубока ли покупка, продажа) на момент снимка.
pub type Verdicts = HashMap<PoolRef, (bool, bool)>;

/// Какая настройка глубины не задана (её имя из ошибки хранилища — GatewayError.setting, не текст): для перепроверки
/// ждущих итогов именно при её задании (settings_wait.rs). None — все заданы. Ошибок нет.
pub fn missing_setting(ctx: &Ctx) -> Option<String> {
    [
        "dex.min_pool_liquidity_usd",
        "dex.depth_band_pct",
        "dex.depth_hysteresis_pct",
    ]
    .into_iter()
    .find_map(|n| ctx.dec(n).err().and_then(|e| e.setting))
}

/// Гистерезис (см. шапку): `prev` — прошлый вердикт (None — первого не было). Возвращает: глубокое ли направление.
/// Ошибок нет.
pub fn next_deep(prev: Option<bool>, depth: Decimal, min: Decimal, hyst_pct: Decimal) -> bool {
    match prev {
        Some(true) => depth >= min * (Decimal::ONE - hyst_pct / Decimal::ONE_HUNDRED),
        _ => depth >= min,
    }
}

impl Book {
    /// Кого будить, когда направление пула стало глубоким (сигнал «пул готов» подписки на рынок). Ошибок нет.
    pub fn on_rise(&self, n: Arc<Notify>) {
        let mut w = self.wake.lock().unwrap_or_else(|p| p.into_inner());
        w.retain(|x| x.strong_count() > 0);
        w.push(Arc::downgrade(&n));
    }

    /// Решение 217 (исправление по проверке): вердикты гистерезиса пулов снимка — копия в момент снимка (одна короткая
    /// блокировка, десятки поисков), а не чтение живой книги в момент фонового расчёта.
    /// Принимает: ссылки пулов снимка. Возвращает: вердикты пулов, у которых он уже был. Ошибок нет.
    pub fn verdicts<'a>(&self, refs: impl Iterator<Item = &'a PoolRef>) -> Verdicts {
        let m = self.read();
        refs.filter_map(|r| {
            m.get(r)
                .map(|s| (r.clone(), (s.depth.deep_buy, s.depth.deep_sell)))
        })
        .collect()
    }

    /// Решение 217: книга для замороженного реестра (registry_frozen.rs): пуста, прошлый вердикт гистерезиса — из
    /// вердиктов момента снимка (verdicts), смены не пишутся в журнал и «пул готов» не подаётся. Ошибок нет.
    pub fn seeded(v: Verdicts) -> Book {
        Book {
            seed: Some(Arc::new(v)),
            ..Book::default()
        }
    }

    /// Решение 301 (б): сколько раз книга считала глубину по состоянию пула. Ошибок нет.
    pub fn calcs(&self) -> u64 {
        self.calcs.load(Ordering::Relaxed)
    }

    /// Последняя посчитанная глубина пула (без пересчёта). Ошибок нет.
    pub fn last(&self, r: &PoolRef) -> Option<Depth> {
        self.read().get(r).map(|s| s.depth.clone())
    }

    /// Глубина пула на его текущем состоянии (см. шапку). Принимает: окружение, реестр, токены, запись, знаки USDT.
    /// Возвращает: глубину. Ошибки (текст): не задана настройка глубины, значение вне допустимого, глубина не
    /// считается (quote/depth_band.rs).
    pub fn depth(
        &self,
        ctx: &Ctx,
        reg: &Registry,
        tokens: &Tokens,
        e: &Entry,
        usdt_dec: u8,
    ) -> Result<Depth, String> {
        let p = Self::prep(ctx, tokens)?;
        self.depth_prep(ctx, reg, tokens, e, usdt_dec, &p)
    }

    /// Подготовка к расчёту глубины многих пулов подряд (решение 131, pools/graph.rs build): настройки глубины —
    /// один раз. Ошибки (текст): не задана настройка глубины, гистерезис вне 0..100.
    pub fn prep(ctx: &Ctx, tokens: &Tokens) -> Result<Prep, String> {
        let read = |n: &str| ctx.dec(n).map_err(|e| e.message);
        let (band_pct, min) = (
            read("dex.depth_band_pct")?,
            read("dex.min_pool_liquidity_usd")?,
        );
        let hyst = read("dex.depth_hysteresis_pct")?;
        if hyst.is_sign_negative() || hyst >= Decimal::ONE_HUNDRED {
            return Err(format!(
                "dex.depth_hysteresis_pct = {hyst}: нужно от 0 и меньше 100"
            ));
        }
        Ok(Prep::new(ctx, tokens, band_pct, min, hyst))
    }

    /// Глубина пула при готовой подготовке (prep) — то же, что depth. Ошибки: глубина не считается (depth_band.rs).
    pub fn depth_prep(
        &self,
        ctx: &Ctx,
        reg: &Registry,
        tokens: &Tokens,
        e: &Entry,
        usdt_dec: u8,
        p: &Prep,
    ) -> Result<Depth, String> {
        self.depth_prep_r(ctx, reg, tokens, e, usdt_dec, p, &mut false)
    }

    /// То же, что depth_prep, и признак `recomputed` (решение 301 д — замер построения графа): true — глубина считалась
    /// заново (не взята из памяти — ни удачная, ни неудачная). Ошибки: как у depth_prep.
    #[allow(clippy::too_many_arguments)]
    pub fn depth_prep_r(
        &self,
        ctx: &Ctx,
        reg: &Registry,
        tokens: &Tokens,
        e: &Entry,
        usdt_dec: u8,
        p: &Prep,
        recomputed: &mut bool,
    ) -> Result<Depth, String> {
        let (band_pct, min, hyst) = (p.band, p.min, p.hyst);
        let key = p.key(reg, e);
        let r = &e.pool.reference;
        let uk = UnitsKey {
            state_gen: e.state_gen,
            band: band_pct,
        };
        // Решение 336: поиск — под чтением (тот же поиск и копия, что раньше под Mutex).
        let (prev, memo) = match self.read().get(r) {
            Some(s) if s.key == key => return Ok(s.depth.clone()),
            Some(s) => (
                Some((s.depth.deep_buy, s.depth.deep_sell)),
                s.units
                    .as_ref()
                    .filter(|(k, _)| k.same(&uk))
                    .map(|(_, u)| *u),
            ),
            None => (self.seed.as_ref().and_then(|v| v.get(r).copied()), None),
        };
        // Решение 307: кривая пула (band_units) — из памяти по (поколение, полоса), если сменился только курс встречного
        // токена или порог/гистерезис; иначе — расчёт (решение 301 б: неудача на том же UnitsKey — та же причина).
        let units = match memo {
            Some(u) => u,
            None => self.units(e, uk, recomputed)?,
        };
        // Ревью №1: признак `recomputed` (R замера 301 д) — как до решения 307: только расчёт кривой (depth_units.rs);
        // перевод в USD по новому курсу при кривой из памяти пересчётом не считается.
        // Решение 307: пулы «встречный — USDT» — из подготовки (одно чтение реестра на встречный токен).
        let rate = p.rate(reg, e);
        let pools = rate.as_ref().map(|x| x.pools.as_slice());
        let band = depth_band::band_from_rate(ctx, reg, tokens, e, usdt_dec, units, pools)?;
        let deep_buy = next_deep(prev.map(|p| p.0), band.buy, min, hyst);
        let deep_sell = next_deep(prev.map(|p| p.1), band.sell, min, hyst);
        let depth = Depth {
            band,
            band_pct,
            min,
            hyst,
            deep_buy,
            deep_sell,
        };
        let rose = prev.is_some_and(|p| (!p.0 && deep_buy) || (!p.1 && deep_sell));
        {
            let mut m = self.lock();
            // Ревью №1 (M): слот уже посчитан на более новом состоянии пула из реестра (поколение больше) — более старая
            // копия записи реестра его не затирает: глубина отдаётся вызывающему, слот, журнал смен и «пул готов» — по
            // более новому состоянию. Раньше get и insert шли под разными блокировками, и расчёт по старой копии
            // (select.rs, полная сборка графа) перезаписывал слот — вердикт гистерезиса «прыгал», а самопроверка графа
            // видела ложное расхождение. Поколение 0 (запись вне реестра: проверки, предторговая сверка, тесты) с
            // поколениями реестра не сравнимо — пишется, как до правки; смена настроек или курса при том же поколении —
            // тоже пишется (сравнение строгое).
            if e.state_gen != 0 && m.get(r).is_some_and(|s| s.key.state_gen() > e.state_gen) {
                return Ok(depth);
            }
            let said = m.remove(r).map(|s| s.said).unwrap_or_default();
            let label = format!(
                "{} {} ({})",
                e.family.title,
                e.ticker,
                label(&e.pool.reference)
            );
            m.insert(
                r.clone(),
                Slot {
                    depth: depth.clone(),
                    key,
                    units: Some((uk, units)),
                    said,
                    label,
                },
            );
        }
        if self.seed.is_none() {
            depth_log::after(self, ctx, r);
            if rose {
                self.rise();
            }
        }
        Ok(depth)
    }

    /// Ревью №1 (P): глубина в USD всех пулов пересчитывается при следующем запросе (dex.v2_fee_pct — комиссия пары V2
    /// входит в курс встречного токена, но не в ключ): ключи слотов помечаются устаревшими; вердикт гистерезиса и кривая
    /// (band_units — без комиссии) остаются. Вызывает фоновый владелец графа при смене настройки. Ошибок нет.
    pub fn invalidate(&self) {
        for s in self.lock().values_mut() {
            s.key.invalidate();
        }
    }

    /// Направление стало глубоким — «пул готов» всем, кто ждёт. Ошибок нет.
    fn rise(&self) {
        let w = self.wake.lock().unwrap_or_else(|p| p.into_inner());
        for n in w.iter().filter_map(Weak::upgrade) {
            n.notify_waiters();
        }
    }

    /// Запись (пересчёт, журнал смен, сброс ключей). Ошибок нет.
    pub(super) fn lock(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<PoolRef, Slot>> {
        self.m.write().unwrap_or_else(|p| p.into_inner())
    }

    /// Решение 336: чтение (попадание по ключу, последняя глубина, вердикты). Ошибок нет.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<PoolRef, Slot>> {
        self.m.read().unwrap_or_else(|p| p.into_inner())
    }
}

/// Адрес пула или PoolId (V4) — для текстов. Ошибок нет.
pub fn label(r: &PoolRef) -> String {
    match r {
        PoolRef::Contract(a) => a.to_string(),
        PoolRef::Singleton { id, .. } => id.to_string(),
        PoolRef::Member { contract, coin } => format!("{contract}/{coin}"),
        // Решение 295: пара многомонетного пула — контракт и пара.
        PoolRef::Multi {
            contract,
            token0,
            token1,
        } => format!("{contract}/{token0}/{token1}"),
    }
}
