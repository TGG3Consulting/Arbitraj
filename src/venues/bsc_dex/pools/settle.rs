//! Файл: venues/bsc_dex/pools/settle.rs
//! Что делает: решение 357 (задача t154, деньги, НЕ под настройкой) — пул «в обновлении»: пока известно, что логи
//! ОДНОГО действия в сети ещё не все применены, пул не достоверен (valid = false — котировки, граф, проверки и сделки его
//! не берут, quote/select.rs и др.) и ядру не отправляется; ядро держит прежнее (настоящее) состояние пула. Когда
//! недостающие логи применены — достоверность возвращается такой, какой была до отметки (was_valid), и пул уходит ядру
//! один раз. Разбор по всем видам — docs/notes/t154-settle-2026-11-16.md.
//! Когда пул «в обновлении» (Need):
//! • Active(a) — пул из бинов (LB V2.1, Infinity Bin): бины события прочитаны на КОНЦЕ блока события, а активный бин
//!   у узла на конце блока (a) не равен активному бину памяти — по коду контрактов логи этого пула в блоке ещё идут
//!   (LB: Swap на каждый пройденный бин, LBPair.sol L499-L541; активный бин меняет только обмен) — settle_bins.rs;
//! • Follow — лог, за которым по коду контракта в ТОМ ЖЕ вызове всегда идёт ещё лог пула: Algebra Integral SwapFee →
//!   Swap (AlgebraPool.sol _emitSwapEvent, integral-core 1ff43e1: emit SwapFee, затем emit Swap); LB CompositionFees →
//!   DepositedToBins (LBPair.sol v2.1.0 mint: CompositionFees в _updateBin цикла, DepositedToBins в конце mint).
//! Отметка head (решение 290 / ревью 332, п. 2 «найдено»): чтение LB на голове не прочиталось (per_block_lb.rs) — пул
//! недостоверен до удачного чтения головы (lb_head.rs put). Раньше удачное чтение головы ставило valid = true ЛЮБОМУ
//! пулу (load::renewed), в т. ч. ждущему полного перечитывания после StaleData (решение 298) с неперечитанными бинами
//! события (вывод из бина, не входящего в активный, — память с лишней ликвидностью, завышение). Теперь голова
//! возвращает достоверность только тому, что сняла сама (was_valid отметки head).
//! Пределы: Active/Follow закрывает приход недостающих логов, иначе — голова сети выше блока отметки при пустом канале
//! событий пачки (settle_close.rs: полное чтение пула на голове).
//! Отметка head — settle_head.rs (head_failed, head_ok).
//! Кто вызывает: pools/listen_log.rs (step, forget), settle_bins.rs, settle_close.rs, settle_head.rs, per_block_lb.rs
//! (pending).
//! От чего зависит: registry.rs, events.rs (Applied), families.rs, abi_lb.rs, abi_algebra_integral.rs.
//! Настройки: нет (базовое поведение; предел — блоки сети, не время).
//!
//! Замки: свой std::sync::Mutex карты отметок и замок реестра никогда не берутся вложенно (сначала реестр, потом карта,
//! по очереди); .await под замком нет.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;

use crate::core::types::{PoolRef, PoolState};

use super::abi_algebra_integral::IAlgebraIntegralPool as Integral;
use super::abi_lb::ILBPair as Lb;
use super::events::Applied;
use super::families::{Family, Proto};
use super::registry::{Entry, Registry};

/// Чего ждёт пул «в обновлении».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Активного бина конца блока: пока активный бин памяти не равен этому — логи блока пула ещё идут.
    Active(u32),
    /// Следующего лога пула того же вызова (по коду контракта он всегда есть).
    Follow,
}

/// Отметка пула.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    /// Блок последнего лога, после которого пул «в обновлении».
    pub block: u64,
    /// Достоверность пула до отметки — её и возвращает снятие.
    pub was_valid: bool,
    /// Бины применённых, но ещё не перечитанных логов (пул из бинов) — читаются одной пачкой при снятии.
    pub bins: BTreeSet<u32>,
    /// Чего ждёт пул; None — только отметка head.
    pub need: Option<Need>,
    /// Чтение LB на голове не прочиталось (per_block_lb.rs) — достоверность возвращает только удачная голова.
    pub head: bool,
}

/// Карта отметок по пулам.
pub type Map = HashMap<PoolRef, Hold>;

/// Отметки пулов (одни на подписку на рынок — Pools). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Holds {
    map: Arc<Mutex<Map>>,
    /// Сколько отметок — проверка «нет ни одной» без замка (горячий путь событий).
    n: Arc<AtomicUsize>,
}

/// Активный бин записи пула из бинов (LB — LbBin, Infinity Bin — Bin); иначе None. Ошибок нет.
pub fn active(e: &Entry) -> Option<u32> {
    match &e.pool.state {
        PoolState::LbBin(d) => Some(d.active_id),
        PoolState::Bin { active_id, .. } => Some(*active_id),
        _ => None,
    }
}

/// Вид из бинов (его отметки снимает settle_bins.rs). Ошибок нет.
pub fn bin_kind(f: &Family) -> bool {
    matches!(f.proto, Proto::LbV21 | Proto::InfinityBin)
}

/// Лог, за которым в том же вызове по коду контракта всегда идёт ещё лог пула (см. шапку). Принимает: вид, лог,
/// итог применения. Возвращает: Some(бины для перечитывания) — открывает Follow; None — нет. Ошибок нет.
pub fn opens(f: &Family, log: &Log, applied: &Applied) -> Option<Vec<u32>> {
    let t0 = log.topic0()?;
    match (f.proto, applied) {
        // Решение 325 (слияние с t152): SwapFee с «добавкой плагина» — Applied::Fee (комиссия читается на блоке
        // события, listen_log.rs зовёт step после чтения) — тоже до Swap того же вызова.
        (Proto::AlgebraIntegral, Applied::Updated | Applied::Fee(_))
            if *t0 == Integral::SwapFee::SIGNATURE_HASH =>
        {
            Some(Vec::new())
        }
        (Proto::LbV21, Applied::Bins(ids)) if *t0 == Lb::CompositionFees::SIGNATURE_HASH => {
            Some(ids.clone())
        }
        _ => None,
    }
}

impl Holds {
    fn lock(&self) -> MutexGuard<'_, Map> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Изменение карты с пересчётом числа отметок (ещё settle_head.rs). Ошибок нет.
    pub(super) fn with<T>(&self, f: impl FnOnce(&mut Map) -> T) -> T {
        let mut m = self.lock();
        let out = f(&mut m);
        self.n.store(m.len(), Ordering::Release);
        out
    }

    /// Есть ли хоть одна отметка (без замка). Ошибок нет.
    pub fn any(&self) -> bool {
        self.n.load(Ordering::Acquire) > 0
    }

    /// Отметка пула (копия); нет отметок — без замка. Ошибок нет.
    pub fn get(&self, r: &PoolRef) -> Option<Hold> {
        if !self.any() {
            return None;
        }
        self.lock().get(r).cloned()
    }

    /// Пул «в обновлении» (ждёт логов: Active / Follow). Ошибок нет.
    pub fn pending(&self, r: &PoolRef) -> bool {
        self.get(r).is_some_and(|h| h.need.is_some())
    }

    /// Пул — «в обновлении»: недостоверен, ждёт `need`; бины `bins` добавляются к неперечитанным (`reset` — сначала
    /// очистить: прежние уже прочитаны). Достоверность до первой отметки запоминается. Пула нет в реестре — ничего.
    /// Ошибок нет.
    pub fn hold(&self, reg: &Registry, r: &PoolRef, at: (u64, Need), bins: Vec<u32>, reset: bool) {
        let Some(was) = reg.update(r, |e| std::mem::replace(&mut e.valid, false)) else {
            return;
        };
        self.with(|m| {
            let h = m.entry(r.clone()).or_insert_with(|| Hold {
                block: at.0,
                was_valid: was,
                bins: BTreeSet::new(),
                need: None,
                head: false,
            });
            h.block = at.0;
            h.need = Some(at.1);
            if reset {
                h.bins.clear();
            }
            h.bins.extend(bins);
        });
    }

    /// Недостающие логи применены (состояние памяти — конец блока / конец вызова): ожидание снято; без отметки head —
    /// отметка удаляется и достоверность возвращается прежней (was_valid; недостоверный по другой причине так и
    /// остаётся). Ошибок нет.
    pub fn release(&self, reg: &Registry, r: &PoolRef) {
        if !self.any() {
            return;
        }
        let back = self.with(|m| match m.get(r).map(|h| h.head) {
            Some(true) => {
                if let Some(h) = m.get_mut(r) {
                    h.need = None;
                    h.bins.clear();
                }
                None
            }
            Some(false) => m.remove(r).map(|h| h.was_valid),
            None => None,
        });
        if back == Some(true) {
            reg.update(r, |e| e.valid = true);
        }
    }

    /// Отметка удаляется без смены достоверности: пул прочитан целиком (Refresh, полное чтение на голове) или
    /// недостоверен по ошибке чтения (его ведёт свой путь). Ошибок нет.
    pub fn forget(&self, r: &PoolRef) {
        if self.any() {
            self.with(|m| m.remove(r));
        }
    }

    /// Лог пула перед разбором ветки (listen_log.rs): открывающий Follow лог — пул «в обновлении» (true — ядру не
    /// отправлять); следующий лог пула не из бинов, ждавшего Follow, — ожидание снято. Пул ждёт полного перечитывания
    /// (`waiting`, решение 298) — ничего (его ведёт повтор). Принимает: реестр, (ссылка, вид), лог, итог применения,
    /// ждёт ли пул повтора. Возвращает: true — пул «в обновлении», ядру не отправлять. Ошибок нет.
    pub fn step(
        &self,
        reg: &Registry,
        at: (&PoolRef, &Family),
        log: &Log,
        a: &Applied,
        waiting: bool,
    ) -> bool {
        let (r, f) = at;
        if waiting {
            return false;
        }
        if let Some(bins) = opens(f, log, a) {
            let block = log.block_number.unwrap_or_default();
            self.hold(reg, r, (block, Need::Follow), bins, false);
            return true;
        }
        if !bin_kind(f) && self.get(r).is_some_and(|h| h.need == Some(Need::Follow)) {
            self.release(reg, r);
        }
        false
    }
}
