//! Файл: venues/bsc_dex/pools/registry.rs
//! Что делает: реестр пулов адаптера в памяти — пул в формате ядра плюс то, что нужно только адаптеру:
//! монета и встречный токен пула (USDT или промежуточная монета), ключ V4, карта тиков, резервы бинов, блок синхронизации, достоверность.
//! Кто вызывает: pools/market.rs, pools/listen.rs, pools/events_*.rs (пишут), quote/*, check/*, exec/* (читают).
//! От чего зависит: core/types (Pool, PoolRef), pools/families.rs, key.rs, ticks.rs, bins.rs, registry_index.rs, prefetch_memo.rs (ревью 325:
//! retain забывает вызовы прошлого чтения снятых пулов), fusion_book.rs (решение 463: retain забывает данные оракула).
//! Решение 129: номер версии реестра (растёт при любой записи — put, update существующего пула, retain) — по нему граф маршрутов строится
//! один раз на состояние реестра, а не на каждую котировку (pools/graph.rs cached); pair_marks — отметки синхронизации
//! пулов пары без копирования записей (ключ кэша глубины, pools/depth_key.rs). Записи — за `Arc`, пары и тикеры —
//! по индексам (registry_index.rs): by_pair / by_coin / pair_marks не проходят весь реестр; карты тиков и бинов
//! общие при копии записи (ticks.rs, bins.rs); snapshot — все записи без копирования (граф маршрутов). Блокировка
//! держится только на чтение индекса и копию нужных записей — счёт котировки идёт без неё.
//! Решение 301 (е2): поколение состояния записи (Entry.state_gen, pools/state_gen.rs) — присваивает put / update: новое,
//! только если состояние, тики, бины, достоверность или комиссия пула фактически сменились; pair_marks — поколения пулов
//! пары (ключ глубины). Решение 301 (е3): update пула, которого нет, версию не меняет (реестр не изменился).
//! Решение 308: каждая запись, сменившая версию (put, update существующего, retain), отмечает пул в наборе изменений
//! фонового графа (graph_changes.rs) под замком записи и ДО роста версии; слежение выключено — одно атомарное чтение.
//! Решение 217: замороженный реестр из снимка записей (снимок и сборка — registry_frozen.rs).
//! Решение 339: отметки неудач чтения пулов (pool_fail.rs Fails) — здесь же; put достоверной записи их снимает (без
//! отметок — без замка, ревью t153 Н5), retain снимает отметки удалённых пулов (Н6). Пулы, ни разу не прочитанные
//! (pool_unread.rs, экран «Монеты»), — здесь же; любой put снимает отметку.
//! Решение 403 (t188): запись (put, update существующего, retain) будит построитель снимка отбора мелких пулов
//! (shallow — quote/shallow_memo.rs wrote / wrote_all: отметка пар, после снятия замка); touch — смена допуска мелких пулов как запись (версия,
//! отметка фоновому графу); pair_stamp / pair_arcs — входы отбора пары без копирования записей.
//! Настройки: нет.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use crate::core::types::{Pool, PoolRef};

use super::super::quote::shallow_stamps::Pair as ShallowPair;
use super::bins::BinMap;
use super::families::Family;
use super::key::PoolKey;
use super::registry_index::Pools;
use super::ticks::TickMap;

/// Запись о пуле.
#[derive(Debug, Clone)]
pub struct Entry {
    pub family: &'static Family,
    /// Пул в формате ядра (состояние, блок, крючок).
    pub pool: Pool,
    /// Тикер монеты пула.
    pub ticker: String,
    /// Адрес монеты пула.
    pub coin: Address,
    /// Встречный токен пула: USDT или промежуточная монета (решение 52) — пулы «монета — промежуточная»
    /// и «промежуточная — USDT» (у второй монета — промежуточная, встречный — USDT).
    pub usdt: Address,
    /// Ключ V4 (CL, Bin); у V2/V3 — None.
    pub key: Option<PoolKey>,
    /// V3/CL: карта тиков; иначе None.
    pub ticks: Option<TickMap>,
    /// Bin: резервы бинов; иначе None.
    pub bins: Option<BinMap>,
    /// Последний учтённый блок: состояние верно на конец этого блока (synced_log = None) или сразу после
    /// события с этим logIndex в нём (synced_log = Some).
    pub synced_block: u64,
    /// logIndex последнего применённого события в synced_block; None — учтён весь блок (состояние прочитано на нём).
    pub synced_log: Option<u64>,
    /// false — подписка оборвалась или карта разошлась с событием: данным верить нельзя до перечитывания.
    pub valid: bool,
    /// Пул мельче dex.min_pool_liquidity_usd: слушается только добавление ликвидности (pools/depth.rs), обмены не
    /// приходят — состояние может отставать. Такой пул не котируется (quote/select.rs) до возврата в полную подписку.
    pub asleep: bool,
    /// Решение 301 (е2): поколение состояния (pools/state_gen.rs) — присваивает реестр (put, update); 0 — запись вне
    /// реестра (её глубина всегда считается заново).
    pub state_gen: u64,
}

/// Реестр пулов. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Registry {
    /// Записи за `Arc` и индексы по паре и тикеру (registry_index.rs, решение 129).
    pub(super) inner: Arc<RwLock<Pools>>,
    /// Глубина пулов по направлениям (решение 73, depth_book.rs).
    pub(super) depth: super::depth_book::Book,
    /// Версия: растёт при любой записи (решение 129).
    pub(super) version: Arc<AtomicU64>,
    /// Граф маршрутов последней версии (решение 129, graph.rs cached).
    pub(super) graph: super::graph::Cache,
    /// Решение 301 (е2): последнее выданное поколение состояния.
    pub(super) gens: Arc<AtomicU64>,
    /// Решение 339: неудачи чтения пулов (причина, пауза повтора — pool_fail.rs); снимаются при put достоверной записи.
    pub(super) fails: super::pool_fail::Fails,
    /// Решение 339 (экран «Монеты»): пулы, ни разу не прочитанные (pool_unread.rs); снимаются при любом put.
    pub(super) unread: super::pool_unread::Book,
    /// Решение 357 (ревью слияния): отметки «в обновлении» и «нужно полное чтение» (settle.rs); put достоверной записи
    /// (полное чтение) снимает отметки пула, retain — отметки удалённых.
    pub(super) holds: super::settle::Holds,
    /// Решение 382 (ревью 2, Н2): учёт доказательств «память неверна» — одно на (пул, позиция) (proof_memo.rs).
    pub(super) proofs: super::proof_memo::Memo,
    /// Решение 393 (повторное ревью t178, Н-B1/Н-B2): память отбора мелких пулов пары (quote/shallow_memo.rs).
    pub(super) shallow: super::super::quote::shallow_memo::Memo,
}

impl Entry {
    /// Учтено ли уже событие с позицией (блок, logIndex). Порядок событий — (blockNumber, logIndex)
    /// (execution-apis src/schemas/receipt.yaml, Log.logIndex — «index of the log in the block»).
    /// Ошибок нет.
    pub fn has_applied(&self, block: u64, log_index: u64) -> bool {
        block < self.synced_block
            || (block == self.synced_block && self.synced_log.is_none_or(|i| log_index <= i))
    }
}

impl Registry {
    /// Глубина пулов по направлениям (решение 73). Ошибок нет.
    pub fn depth(&self) -> &super::depth_book::Book {
        &self.depth
    }

    /// Память отбора мелких пулов пары (решение 393, quote/shallow_memo.rs). Ошибок нет.
    pub fn shallow_memo(&self) -> &super::super::quote::shallow_memo::Memo {
        &self.shallow
    }

    /// Версия реестра (решение 129): растёт при любой записи. Ошибок нет.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Решение 357: отметки «в обновлении» и «нужно полное чтение» (settle.rs). Ошибок нет.
    pub fn holds(&self) -> &super::settle::Holds {
        &self.holds
    }

    /// Решение 382: учёт доказательств «память неверна» (proof_memo.rs). Ошибок нет.
    pub fn proofs(&self) -> &super::proof_memo::Memo {
        &self.proofs
    }

    /// Решение 339: отметки неудач чтения пулов (pool_fail.rs). Ошибок нет.
    pub fn fails(&self) -> &super::pool_fail::Fails {
        &self.fails
    }

    /// Решение 339 (экран «Монеты»): пулы, ни разу не прочитанные (pool_unread.rs). Ошибок нет.
    pub fn unread(&self) -> &super::pool_unread::Book {
        &self.unread
    }

    /// Кэш графа маршрутов (решение 129, graph.rs). Ошибок нет.
    pub fn graph_cache(&self) -> &super::graph::Cache {
        &self.graph
    }

    /// Чтение под блокировкой (короткое: только индекс и копии нужных записей). Ошибок нет.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Pools> {
        self.inner.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Добавляет или заменяет запись. Поколение (решение 301 е2): прежнее, если состояние фактически то же, иначе новое.
    /// Ошибок нет.
    pub fn put(&self, mut e: Entry) {
        // Решение 339: достоверная запись — пул прочитан, отметка неудачи (pool_fail.rs) снимается; ревью t153 (Н5):
        // отметок нет (339 выключено) — без замка карты отметок (атомарный счётчик в Fails::clear).
        if e.valid {
            self.fails.clear(&e.pool.reference);
            // Решение 357 (ревью слияния, Б1): достоверная запись кладётся только полным чтением (lb_head.rs и
            // settle.rs достоверность так не поднимают) — отметки «в обновлении» / «нужно полное чтение» долой.
            self.holds.forget(&e.pool.reference);
        }
        // Пул записан в реестр — экран видит его записью; отметка «ни разу не прочитан» (pool_unread.rs) долой.
        self.unread.clear(&e.pool.reference);
        let mut m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        e.state_gen = match m.get(&e.pool.reference) {
            Some(old) if old.state_gen != 0 && super::state_gen::same(old, &e) => old.state_gen,
            _ => self.next_gen(),
        };
        // Решение 308: отметка для фонового графа — под замком записи и до роста версии (graph_changes.rs, И4).
        self.graph.bg().changes.mark(&e.pool.reference);
        let pair = (e.coin, e.usdt);
        m.insert(e);
        self.version.fetch_add(1, Ordering::AcqRel);
        drop(m);
        // Решение 403: отметка пары построителю отбора мелких пулов (quote/shallow_build.rs) — после записи; предел не
        // задан — одно атомарное чтение.
        self.shallow.wrote(&[pair]);
    }

    /// Копия записи. None — пула нет.
    pub fn get(&self, r: &PoolRef) -> Option<Entry> {
        self.read().get(r).map(|e| (**e).clone())
    }

    /// Решение 308: запись без копирования (Arc) — оценка пула фоновым графом (graph_eval.rs); решение 310 — чтение
    /// достоверности циклами проверок без записи (check/fresh.rs). None — пула нет.
    pub fn entry(&self, r: &PoolRef) -> Option<Arc<Entry>> {
        self.read().get(r).cloned()
    }

    /// Изменение записи на месте. Принимает: ссылку и функцию. Возвращает: её результат; None — пула нет.
    /// Поколение (решение 301 е2): новое, только если функция фактически сменила состояние, тики, бины, достоверность
    /// или комиссию. Решение 301 (е3): пула нет — версия не растёт.
    pub fn update<T>(&self, r: &PoolRef, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        let mut m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        // Решение 403: пара записи до и после изменения (функция может сменить монету или встречный токен).
        let mut seen: (Option<ShallowPair>, Option<ShallowPair>) = (None, None);
        let out = m.update(r, |e| {
            seen.0 = Some((e.coin, e.usdt));
            let before = super::state_gen::Snap::of(e);
            let out = f(e);
            seen.1 = Some((e.coin, e.usdt));
            if e.state_gen == 0 || !before.same(e) {
                e.state_gen = self.next_gen();
            }
            out
        });
        // Версия — до снятия блокировки: читатель, увидевший новое состояние, увидит и новую версию. Решение 301 (е3):
        // пула нет — реестр не изменился, версия прежняя.
        if out.is_some() {
            // Решение 308: отметка для фонового графа — под замком записи и до роста версии (И4).
            self.graph.bg().changes.mark(r);
            self.version.fetch_add(1, Ordering::AcqRel);
        }
        drop(m);
        // Решение 403: отметка пар построителю отбора мелких пулов (quote/shallow_build.rs).
        match seen {
            (Some(a), Some(b)) if a != b => self.shallow.wrote(&[a, b]),
            (Some(a), _) => self.shallow.wrote(&[a]),
            _ => {}
        }
        out
    }

    /// Новое поколение состояния (решение 301 е2): не 0, растёт. Ошибок нет.
    fn next_gen(&self) -> u64 {
        self.gens.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Пулы пары «монета — встречный токен» (копии; карты тиков и бинов — общие, за `Arc`). По индексу пары, а не
    /// проходом реестра (решение 129). Ошибок нет.
    pub fn by_pair(&self, coin: Address, quote: Address) -> Vec<Entry> {
        self.read()
            .pair(coin, quote)
            .map(|e| (**e).clone())
            .collect()
    }

    /// Решение 403: пулы пары «монета — встречный токен» без копирования записей (`Arc`) — отбор мелких пулов
    /// (quote/shallow_pick.rs). Ошибок нет.
    pub fn pair_arcs(&self, coin: Address, quote: Address) -> Vec<Arc<Entry>> {
        self.read().pair(coin, quote).cloned().collect()
    }

    /// Решение 403: (поколение состояния, спит) пулов пары по возрастанию — отпечаток входов отбора мелких пулов
    /// (quote/shallow_memo.rs Stamp), без копирования записей. Ошибок нет.
    pub fn pair_stamp(&self, coin: Address, quote: Address) -> Vec<(u64, bool)> {
        let mut v: Vec<(u64, bool)> = self
            .read()
            .pair(coin, quote)
            .map(|e| (e.state_gen, e.asleep))
            .collect();
        v.sort_unstable();
        v
    }

    /// Поколения состояния пулов пары «монета — встречный токен» (решение 301 е2; раньше — блок, logIndex, достоверен)
    /// — без копирования записей (решение 129: ключ кэша глубины считается для каждого пула при построении графа).
    /// Достоверность входит в поколение. Ошибок нет.
    pub fn pair_marks(&self, coin: Address, quote: Address) -> Vec<u64> {
        self.read().pair(coin, quote).map(|e| e.state_gen).collect()
    }

    /// Все пулы монеты (копии) — по адресу контракта (решение 211; раньше по тикеру). Ошибок нет.
    pub fn by_coin(&self, coin: &Address) -> Vec<Entry> {
        self.read().coin(coin).map(|e| (**e).clone()).collect()
    }

    /// Есть ли такой пул. Ошибок нет.
    pub fn contains(&self, r: &PoolRef) -> bool {
        self.read().contains(r)
    }

    /// Копии всех записей пулов. Ошибок нет.
    pub fn all(&self) -> Vec<Entry> {
        self.read().values().map(|e| (**e).clone()).collect()
    }

    /// Снимок всех записей без копирования (решение 129: граф маршрутов строится по нему вне блокировки). Ошибок нет.
    pub fn snapshot(&self) -> Vec<Arc<Entry>> {
        self.read().values().cloned().collect()
    }

    /// Ссылки на все пулы. Ошибок нет.
    pub fn refs(&self) -> Vec<PoolRef> {
        self.read().refs().cloned().collect()
    }

    /// Удаляет пулы монет, которых больше нет в подписке. Принимает: функцию «оставить ли». Ошибок нет.
    pub fn retain(&self, keep: impl Fn(&Entry) -> bool) {
        let mut m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        let gone = m.retain(keep);
        // Решение 308: удалённые пулы — отметка для фонового графа под замком записи и до роста версии (И4).
        self.graph.bg().changes.mark_all(&gone);
        self.version.fetch_add(1, Ordering::AcqRel);
        drop(m);
        // Ревью 325 (Н8): вызовы прошлого чтения снятых пулов не держатся в памяти (prefetch_memo.rs).
        super::prefetch_memo::forget(&gone);
        // Ревью t153 (Н6): отметки неудач чтения удалённых пулов (pool_fail.rs) — тоже долой.
        self.fails.forget(&gone);
        // Решение 463 (ревью t236): и данные оракула THENA Fusion (fusion_book.rs).
        super::fusion_book::forget(&gone);
        // Решение 357 (ревью слияния, Н4): и отметки «в обновлении» удалённых пулов.
        self.holds.forget_all(&gone);
        // Решение 403: пулы удалены — построителю отбора мелких пулов полный проход (quote/shallow_build.rs).
        self.shallow.wrote_all();
    }

    /// Решение 403: допуск мелких пулов `refs` сменился (новый снимок отбора, quote/shallow_build.rs) — как запись:
    /// отметка фоновому графу под замком записи и до роста версии (решение 308, И4), версия растёт — граф и кэши
    /// котировки по версии пересобираются с новым допуском. Записи не меняются. Ошибок нет.
    pub fn touch(&self, refs: &[PoolRef]) {
        let m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        self.graph.bg().changes.mark_all(refs);
        self.version.fetch_add(1, Ordering::AcqRel);
        drop(m);
    }
}
