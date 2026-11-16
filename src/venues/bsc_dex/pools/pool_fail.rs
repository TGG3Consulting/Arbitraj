//! Файл: venues/bsc_dex/pools/pool_fail.rs
//! Что делает: решение 339 — ошибка чтения ОДНОГО пула касается только этого пула. Задана dex.pool_fail_pause_ms —
//! любая ошибка перечитывания пула (кроме обрыва связи и закрытого канала ядра) не рвёт сессию пачки подписки: пул
//! недостоверен (в котировки и сделки не идёт, «нет данных — не торгуем»), причина — здесь (экран «Монеты»,
//! depth_facts.rs, и журнал: одна запись на начало неудач и на смену причины), повтор — по расписанию: не раньше
//! паузы, которая растёт вдвое с каждой неудачей подряд до dex.pool_fail_pause_max_ms (постоянная ошибка вроде отката
//! контракта не повторяется вечно с полной частотой). Предел попыток очереди повтора — прежний dex.pool_load_retries
//! (reload.rs: брошенный пул читается снова только по своему событию — и тоже не раньше паузы). Остальные пулы пачки
//! работают без переподключения и без перечитывания. Отметка снимается при удачном чтении (registry.rs put).
//! Не задана dex.pool_fail_pause_ms — всё 1 в 1 как раньше: отметок нет, пауз нет, прочая ошибка рвёт сессию пачки.
//! Кто вызывает: pools/listen_log.rs, listen_site.rs, reload.rs (retry), per_block.rs, per_block_lb.rs,
//! per_block_balancer.rs, depth_facts.rs (причина на экран), registry.rs (хранит отметки, снимает при put).
//! От чего зависит: ctx.rs (настройки), listen.rs (Pools), registry.rs, discover.rs (Found), storage/journal.
//! Настройки: dex.pool_fail_pause_ms, dex.pool_fail_pause_max_ms [bsc_dex] (необязательные).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::discover::Found;
use super::listen::Pools;

/// Настройка: начальная пауза повтора пула после ошибки чтения; задана — ошибка пула не рвёт сессию пачки.
pub const PAUSE: &str = "dex.pool_fail_pause_ms";
/// Настройка: потолок растущей паузы.
pub const PAUSE_MAX: &str = "dex.pool_fail_pause_max_ms";

/// Неудачи чтения одного пула подряд.
#[derive(Debug, Clone)]
pub struct Fail {
    /// Причина последней неудачи (текст ошибки узла / разбора).
    pub why: String,
    /// Сколько неудач подряд.
    pub tries: u32,
    /// Текущая пауза, мс.
    pub pause_ms: u64,
    /// Не раньше этого момента — следующее чтение.
    pub next: Instant,
}

/// Отметки неудач пулов площадки. Дешёво клонируется (хранится в реестре, registry.rs).
#[derive(Clone, Default)]
pub struct Fails {
    map: Arc<Mutex<HashMap<PoolRef, Fail>>>,
}

impl Fails {
    /// Короткий замок карты. Ошибок нет.
    fn lock(&self) -> MutexGuard<'_, HashMap<PoolRef, Fail>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Что делает: отметка пула (копия). None — неудач нет. Ошибок нет.
    pub fn get(&self, r: &PoolRef) -> Option<Fail> {
        self.lock().get(r).cloned()
    }

    /// Что делает: снимает отметку (пул прочитан). Ошибок нет.
    pub fn clear(&self, r: &PoolRef) {
        self.lock().remove(r);
    }

    /// Что делает: пора ли читать пул: отметки нет или пауза прошла. Принимает: ссылку, «сейчас». Ошибок нет.
    pub fn due(&self, r: &PoolRef, now: Instant) -> bool {
        self.lock().get(r).is_none_or(|f| now >= f.next)
    }

    /// Что делает: ещё одна неудача пула: число неудач +1, пауза — pause_after, следующее чтение — через неё.
    /// Принимает: ссылку, причину, начальную паузу и потолок (мс), «сейчас». Возвращает: новую отметку и true, если
    /// это первая неудача или причина сменилась (тогда пишется журнал). Ошибок нет.
    pub fn record(
        &self,
        r: &PoolRef,
        why: &str,
        base: u64,
        max: u64,
        now: Instant,
    ) -> (Fail, bool) {
        let mut m = self.lock();
        let (tries, fresh) = match m.get(r) {
            Some(f) => (f.tries.saturating_add(1), f.why != why),
            None => (1, true),
        };
        let pause_ms = pause_after(base, max, tries);
        let f = Fail {
            why: why.to_string(),
            tries,
            pause_ms,
            next: now + Duration::from_millis(pause_ms),
        };
        m.insert(r.clone(), f.clone());
        (f, fresh)
    }
}

/// Что делает: пауза после `tries` неудач подряд: base · 2^(tries−1), не больше потолка (потолок меньше base — base).
/// Принимает: начальную паузу, потолок (мс), число неудач (0 — как 1). Возвращает: паузу, мс. Ошибок нет.
pub fn pause_after(base: u64, max: u64, tries: u32) -> u64 {
    let shift = tries.saturating_sub(1).min(63);
    base.saturating_mul(1u64 << shift).min(max.max(base))
}

/// Что делает: включена ли изоляция ошибок пула (решение 339). Возвращает: Some(начальная пауза, мс) — задана и > 0;
/// None — не задана (прежнее поведение). Ошибок нет.
pub fn base(ctx: &Ctx) -> Option<u64> {
    ctx.int(PAUSE).ok().filter(|&n| n > 0)
}

/// Что делает: потолок паузы: dex.pool_fail_pause_max_ms; не задан — пауза не растёт (равна начальной).
/// Принимает: окружение, начальную паузу. Ошибок нет.
pub fn cap(ctx: &Ctx, base: u64) -> u64 {
    ctx.int(PAUSE_MAX).ok().filter(|&n| n > 0).unwrap_or(base)
}

/// Что делает: пул на паузе после неудач (изоляция включена и пауза не прошла) — читать его сейчас нельзя.
/// Принимает: окружение, ссылку. Возвращает: true — на паузе. Ошибок нет.
pub fn paused(p: &Pools, r: &PoolRef) -> bool {
    base(&p.ctx).is_some() && !p.reg.fails().due(r, Instant::now())
}

/// Что делает: неудача чтения пула при включённой изоляции: отметка (пауза растёт), журнал — на первую неудачу и на
/// смену причины. Принимает: окружение, ссылку, имя вида, тикер, причину. Возвращает: true — изоляция включена
/// (вызывающий НЕ рвёт сессию); false — не задана dex.pool_fail_pause_ms (как раньше, ничего не записано). Ошибок нет.
pub async fn note(p: &Pools, r: &PoolRef, title: &str, ticker: &str, why: &str) -> bool {
    let Some(b) = base(&p.ctx) else {
        return false;
    };
    let (f, fresh) = p
        .reg
        .fails()
        .record(r, why, b, cap(&p.ctx, b), Instant::now());
    if fresh {
        let msg = format!(
            "{title} {ticker}: состояние пула не прочитано: {why} — пул недостоверен (в котировки и сделки не идёт), \
             следующее чтение не раньше чем через {} мс (пауза растёт с каждой неудачей); остальные пулы пачки \
             работают без переподключения (решение 339)",
            f.pause_ms
        );
        p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
    }
    true
}

/// Что делает: пул «каждым блоком» (per_block*.rs) не прочитался: недостоверен (как раньше) и, при изоляции, отметка
/// с причиной и паузой (следующие блоки его не читают, пока пауза не прошла). Принимает: окружение, пул, причину.
/// Ошибок нет.
pub async fn block_failed(p: &Pools, f: &Found, why: &str) {
    p.reg.update(&f.reference, |e| e.valid = false);
    let ticker = p
        .reg
        .get(&f.reference)
        .map(|e| e.ticker)
        .unwrap_or_default();
    note(p, &f.reference, f.family.title, &ticker, why).await;
}

/// Что делает: строка причины для экрана «Монеты» (depth_facts.rs details). Принимает: отметку, «сейчас».
/// Возвращает: текст. Ошибок нет.
pub fn detail(f: &Fail, now: Instant) -> String {
    let left = f.next.saturating_duration_since(now).as_millis();
    format!(
        "не прочитан: {} (неудач подряд {}; следующее чтение не раньше чем через {left} мс)",
        f.why, f.tries
    )
}
