//! Файл: venues/bsc_dex/pools/pool_fail.rs
//! Что делает: решение 339 — ошибка чтения ОДНОГО пула касается только этого пула. Задана dex.pool_fail_pause_ms —
//! любая ошибка перечитывания пула (кроме обрыва связи и закрытого канала ядра) не рвёт сессию пачки подписки: пул
//! недостоверен (в котировки и сделки не идёт, «нет данных — не торгуем»), причина — здесь (экран «Монеты»,
//! depth_facts.rs, и журнал), повтор — по расписанию: не раньше паузы, которая растёт вдвое с каждой неудачей пула
//! подряд до dex.pool_fail_pause_max_ms (постоянная ошибка вроде отката контракта не повторяется с полной частотой).
//! Здесь — отметки (Fails, хранятся в реестре) и расчёт паузы; запись неудачи, журнал, уведомление и очередь повтора —
//! pool_fail_note.rs (реэкспорт ниже: прежние пути pool_fail::note / block_failed / paused сохранены).
//! Ревью t153:
//! • Б1 — пул не бросается: при заданной паузе очередь повтора держит пул до удачного чтения (reload.rs), частоту
//!   ограничивают пауза и её потолок; событие недостоверного пула не на паузе ставит его на перечитку (listen_log.rs);
//! • Н4 — ошибка связи или лимита узла (код шлюза Unreachable / Timeout / RateLimited — node/failover.rs switchable)
//!   ошибкой ПУЛА не считается: пауза не растёт (всегда начальная — после восстановления узла пул читается сразу по
//!   истечении начальной паузы, а не потолка), журнал — одна запись на сбой узла (не на каждый пул); причина в журнале
//!   сравнивается по коду ошибки, а не по тексту;
//! • Н3 — момент следующего чтения без паники при огромном потолке (checked_add);
//! • Н5 — быстрый счётчик отметок: без отметок (339 выключено) put реестра не берёт замок карты;
//! • Н6 — отметки удалённых пулов чистятся (registry.rs retain, market.rs keep_only).
//! Не задана dex.pool_fail_pause_ms — всё 1 в 1 как раньше: отметок нет, пауз нет, прочая ошибка рвёт сессию пачки.
//! Кто вызывает: pool_fail_note.rs, depth_facts.rs (причина на экран), registry.rs (хранит отметки, снимает при put,
//! чистит при retain), market.rs (keep_only); через реэкспорт — listen_log.rs, listen_site.rs, reload.rs, per_block*.rs.
//! От чего зависит: ctx.rs (настройки), gateway/errors.rs (ErrorCode), node/failover.rs (switchable).
//! Настройки: dex.pool_fail_pause_ms, dex.pool_fail_pause_max_ms, dex.pool_unread_alert_s [bsc_dex] (необязательные).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::core::types::PoolRef;
use crate::gateway::errors::ErrorCode;

use super::super::ctx::Ctx;

pub use super::pool_fail_note::{Reason, block_failed, note, paused, requeue};

/// Настройка: начальная пауза повтора пула после ошибки чтения; задана — ошибка пула не рвёт сессию пачки.
pub const PAUSE: &str = "dex.pool_fail_pause_ms";
/// Настройка: потолок растущей паузы.
pub const PAUSE_MAX: &str = "dex.pool_fail_pause_max_ms";
/// Настройка (Н7): пул не читается дольше стольких секунд — уведомление dex.pool_unread (одно на эпизод неудач).
pub const ALERT: &str = "dex.pool_unread_alert_s";

/// Неудачи чтения одного пула подряд.
#[derive(Debug, Clone)]
pub struct Fail {
    /// Причина последней неудачи (текст ошибки узла / разбора).
    pub why: String,
    /// Код последней неудачи (None — без кода); по нему — «сменилась ли причина» (журнал).
    pub code: Option<ErrorCode>,
    /// Сколько неудач подряд (любых, и связи узла тоже).
    pub tries: u32,
    /// Сколько неудач ПУЛА подряд (без ошибок связи и лимита узла) — от них растёт пауза.
    pub grow: u32,
    /// Текущая пауза, мс.
    pub pause_ms: u64,
    /// Не раньше этого момента — следующее чтение.
    pub next: Instant,
    /// Первая неудача эпизода (с неё отсчёт dex.pool_unread_alert_s).
    pub since: Instant,
    /// Уведомление dex.pool_unread за этот эпизод уже ушло.
    pub told: bool,
}

/// Отметки неудач пулов площадки. Дешёво клонируется (хранится в реестре, registry.rs).
#[derive(Clone, Default)]
pub struct Fails {
    map: Arc<Mutex<HashMap<PoolRef, Fail>>>,
    /// Н5: сколько отметок (меняется под замком карты) — без отметок clear не берёт замок.
    marks: Arc<AtomicUsize>,
    /// Н4: о сбое связи / лимита узла уже записано в журнал (снимается удачным чтением любого отмеченного пула).
    node: Arc<AtomicBool>,
}

/// Что делает: ошибка связи или лимита узла, а не пула (node/failover.rs switchable: Unreachable — нет связи / 5xx,
/// Timeout, RateLimited — 429). Принимает: код (None — нет). Возвращает: true — узел. Ошибок нет.
pub fn is_link(code: Option<ErrorCode>) -> bool {
    matches!(
        code,
        Some(ErrorCode::Unreachable | ErrorCode::Timeout | ErrorCode::RateLimited)
    )
}

impl Fails {
    /// Короткий замок карты. Ошибок нет.
    fn lock(&self) -> MutexGuard<'_, HashMap<PoolRef, Fail>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Счётчик отметок — после изменения карты (под её замком). Ошибок нет.
    fn sync(&self, m: &HashMap<PoolRef, Fail>) {
        self.marks.store(m.len(), Ordering::Release);
    }

    /// Что делает: отметка пула (копия). None — неудач нет. Ошибок нет.
    pub fn get(&self, r: &PoolRef) -> Option<Fail> {
        self.lock().get(r).cloned()
    }

    /// Что делает: есть ли хоть одна отметка (без замка). Ошибок нет.
    pub fn any(&self) -> bool {
        self.marks.load(Ordering::Acquire) > 0
    }

    /// Что делает: снимает отметку (пул прочитан); отметок нет — без замка (Н5). Снятая отметка снимает и «о сбое узла
    /// записано» (узел снова отвечает). Ошибок нет.
    pub fn clear(&self, r: &PoolRef) {
        if !self.any() {
            return;
        }
        let mut m = self.lock();
        if m.remove(r).is_some() {
            self.sync(&m);
            self.node.store(false, Ordering::Release);
        }
    }

    /// Что делает: Н6 — снимает отметки пулов `gone` (удалены из реестра). Ошибок нет.
    pub fn forget(&self, gone: &[PoolRef]) {
        if !self.any() || gone.is_empty() {
            return;
        }
        let mut m = self.lock();
        for r in gone {
            m.remove(r);
        }
        self.sync(&m);
    }

    /// Что делает: Н6 — оставляет отметки только пулов `known` (найденные подпиской; у не прочитанных ни разу пулов
    /// записи в реестре нет — их отметки чистит только это). Ошибок нет.
    pub fn keep_only(&self, known: &HashSet<PoolRef>) {
        if !self.any() {
            return;
        }
        let mut m = self.lock();
        m.retain(|r, _| known.contains(r));
        self.sync(&m);
    }

    /// Что делает: пора ли читать пул: отметки нет или пауза прошла. Принимает: ссылку, «сейчас». Ошибок нет.
    pub fn due(&self, r: &PoolRef, now: Instant) -> bool {
        if !self.any() {
            return true;
        }
        self.lock().get(r).is_none_or(|f| now >= f.next)
    }

    /// Что делает: ещё одна неудача пула. Неудача пула: grow +1, пауза — pause_after(grow); ошибка связи / лимита узла
    /// (is_link): grow прежний, пауза — начальная. Следующее чтение — через паузу (без паники при огромной паузе —
    /// later, Н3).
    /// Принимает: ссылку, код и текст причины, начальную паузу и потолок (мс), «сейчас». Возвращает: новую отметку и
    /// true, если это первая неудача или сменился код причины (тогда пишется журнал). Ошибок нет.
    pub fn record(
        &self,
        r: &PoolRef,
        (code, why): (Option<ErrorCode>, &str),
        base: u64,
        max: u64,
        now: Instant,
    ) -> (Fail, bool) {
        let link = is_link(code);
        let mut m = self.lock();
        let (tries, grow, since, told, fresh) = match m.get(r) {
            Some(f) => (
                f.tries.saturating_add(1),
                if link {
                    f.grow
                } else {
                    f.grow.saturating_add(1)
                },
                f.since,
                f.told,
                f.code != code,
            ),
            None => (1, u32::from(!link), now, false, true),
        };
        let pause_ms = if link {
            base
        } else {
            pause_after(base, max, grow)
        };
        let f = Fail {
            why: why.to_string(),
            code,
            tries,
            grow,
            pause_ms,
            next: later(now, pause_ms),
            since,
            told,
        };
        m.insert(r.clone(), f.clone());
        self.sync(&m);
        (f, fresh)
    }

    /// Что делает: отмечает, что уведомление dex.pool_unread за эпизод ушло. Возвращает: true — отмечено сейчас
    /// (раньше не уходило); false — уже уходило или отметки нет. Ошибок нет.
    pub fn tell(&self, r: &PoolRef) -> bool {
        let mut m = self.lock();
        match m.get_mut(r) {
            Some(f) if !f.told => {
                f.told = true;
                true
            }
            _ => false,
        }
    }

    /// Что делает: Н4 — первый сбой узла после удачного чтения: true (писать журнал один раз); дальше — false до
    /// удачного чтения отмеченного пула. Ошибок нет.
    pub fn node_down(&self) -> bool {
        !self.node.swap(true, Ordering::AcqRel)
    }
}

/// Что делает: Н3 — «сейчас» + пауза без паники при переполнении Instant: не помещается — пауза делится пополам, пока
/// не поместится (пауза 0 помещается всегда). Принимает: «сейчас», паузу (мс). Ошибок нет.
pub fn later(now: Instant, pause_ms: u64) -> Instant {
    let mut d = Duration::from_millis(pause_ms);
    loop {
        if let Some(t) = now.checked_add(d) {
            return t;
        }
        d /= 2;
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

/// Что делает: Н7 — порог уведомления «пул не читается»: dex.pool_unread_alert_s; не задан — уведомления нет.
/// Принимает: окружение. Возвращает: секунды. Ошибок нет.
pub fn alert_after(ctx: &Ctx) -> Option<u64> {
    ctx.int(ALERT).ok().filter(|&n| n > 0)
}

/// Что делает: строка причины для экрана «Монеты» (depth_facts.rs details). Принимает: отметку, «сейчас».
/// Возвращает: текст. Ошибок нет.
pub fn detail(f: &Fail, now: Instant) -> String {
    let left = f.next.saturating_duration_since(now).as_millis();
    let unread = now.saturating_duration_since(f.since).as_secs();
    format!(
        "не прочитан: {} (неудач подряд {}, не читается {unread} с; следующее чтение не раньше чем через {left} мс)",
        f.why, f.tries
    )
}
