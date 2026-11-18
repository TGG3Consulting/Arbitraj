//! Файл: gateway/addr_book.rs
//! Что делает: решение 411 (задача t196, Тигран 19.11: «адрес депозита — из памяти, но умную связку: если я завтра
//! поменяю API-ключи, бот не должен взять неправильный адрес») — общая память адресов депозита адаптеров бирж,
//! привязанная к учётной записи, и режим запроса адреса на задаче.
//!
//! Память AddrBook: ключ — (отпечаток ключа API биржи, монета, сеть). Отпечаток — secrets/vault.rs mark: хеш значения
//! секрета «<биржа>.api_key» с зерном процесса (сам ключ и секрет не хранятся и не пишутся в журнал). Отпечаток
//! читается из хранилища секретов в момент поиска (Mark::read) — хранилище меняет ключи в памяти одним присваиванием
//! под своим замком (vault.rs write), поэтому сразу после смены ключа адрес прежнего ключа не находится. Кладётся адрес
//! только если отпечаток после ответа биржи тот же, что до запроса (Mark::still): ключ сменили во время запроса —
//! ответ в память не идёт. Секреты заблокированы или ключа нет — из памяти ничего, в память ничего.
//! До решения 411 у каждого из 14 адаптеров бирж был свой HashMap по (монета, сеть) без привязки к ключу.
//!
//! Режим запроса — на задаче tokio (task_local, как addr_claim.rs; ядро о площадках не знает): Normal (вне признака)
//! — как раньше: есть в памяти — из памяти, нет — запрос; Memory (`memory_only`) — только память, нет — NotApplicable;
//! Fresh (`fresh`) — память не читается, всегда запрос, ответ кладётся в память (триггерный запрос ядра,
//! core/deposit_proof.rs). Что делать — чистая логика `decide` (тест ниже).
//!
//! Кто вызывает: venues/*/deposit_addr.rs (14 адаптеров бирж), core/deposit_proof.rs и core/deposit_warm_pass.rs
//! (memory_only, fresh).
//! Повторное ревью 411: is_miss — отказ «нет в памяти» отличим от прочих ошибок адаптера; key_epoch — номер изменения
//! секретов (сверка с памятью — только если секреты за время запроса не менялись).
//! От чего зависит: tokio (task_local), gateway/errors.rs, secrets/vault.rs (mark, generation).
//! Настройки: нет.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::secrets::vault::Vault;

use super::errors::{ErrorCode, GatewayError, GatewayResult};

tokio::task_local! {
    /// Режим запроса адреса депозита в этой задаче.
    static MODE: Mode;
}

/// Режим запроса адреса депозита.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Как раньше: память, иначе запрос.
    Normal,
    /// Только память, без запроса к бирже.
    Memory,
    /// Только запрос к бирже (свежий ответ), память не читается.
    Fresh,
}

/// Что делать адаптеру с памятью.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Look<V> {
    /// Взять адрес из памяти.
    Hit(V),
    /// Запросить биржу.
    Ask,
    /// Нет в памяти, а запрос запрещён (Memory).
    Miss,
}

/// Что делает: выполняет работу в режиме «только память». Принимает: работу. Возвращает: её итог. Ошибок нет.
pub async fn memory_only<F: Future>(work: F) -> F::Output {
    MODE.scope(Mode::Memory, work).await
}

/// Что делает: выполняет работу в режиме «только свежий ответ биржи». Принимает: работу. Возвращает: её итог.
/// Ошибок нет.
pub async fn fresh<F: Future>(work: F) -> F::Output {
    MODE.scope(Mode::Fresh, work).await
}

/// Хвост текста отказа «нет в памяти» (режим Memory) — по нему ядро отличает промах памяти от прочих ошибок адаптера.
const MISS: &str = "нет в памяти (запрос только из памяти)";

/// Что делает: это отказ «адреса нет в памяти» режима Memory (а не другая ошибка адаптера: например адрес в памяти,
/// но зачисляет не на торговый счёт — KuCoin/OKX). Принимает: ошибку. Ошибок нет.
pub fn is_miss(e: &GatewayError) -> bool {
    e.code == ErrorCode::NotApplicable && e.message.ends_with(MISS)
}

/// Что делает: номер изменения секретов процесса (secrets/vault.rs generation): одинаковый до и после запроса —
/// ключи API за это время не менялись. Ошибок нет.
pub fn key_epoch() -> u64 {
    crate::secrets::vault::generation()
}

/// Что делает: режим текущей задачи (вне признака — Normal). Ошибок нет.
pub fn mode() -> Mode {
    MODE.try_with(|m| *m).unwrap_or(Mode::Normal)
}

/// Что делает: чистая логика решения 411 — что делать с найденным в памяти адресом в режиме задачи.
/// Принимает: режим, адрес из памяти (None — нет). Возвращает: Look. Ошибок нет.
pub fn decide<V>(mode: Mode, mem: Option<V>) -> Look<V> {
    match (mode, mem) {
        (Mode::Fresh, _) => Look::Ask,
        (_, Some(v)) => Look::Hit(v),
        (Mode::Memory, None) => Look::Miss,
        (Mode::Normal, None) => Look::Ask,
    }
}

/// Отпечаток ключа API, прочитанный перед запросом адреса.
#[derive(Clone)]
pub struct Mark {
    at: Option<u64>,
    /// Откуда перечитать отпечаток после ответа; None — отпечаток задан готовым (Mark::fixed).
    src: Option<(Vault, &'static str)>,
}

impl Mark {
    /// Что делает: отпечаток секрета `name` сейчас. Принимает: хранилище, имя секрета ключа API. Ошибок нет.
    pub fn read(vault: &Vault, name: &'static str) -> Self {
        Self {
            at: vault.mark(name),
            src: Some((vault.clone(), name)),
        }
    }

    /// Что делает: готовый отпечаток без хранилища (чистая логика и тесты). Ошибок нет.
    pub fn fixed(at: Option<u64>) -> Self {
        Self { at, src: None }
    }

    /// Что делает: отпечаток для записи — тот же, что до запроса (иначе None: ключ сменили во время запроса).
    /// Ошибок нет.
    pub fn still(&self) -> Option<u64> {
        match &self.src {
            Some((vault, name)) => same(self.at, vault.mark(name)),
            None => self.at,
        }
    }
}

/// Что делает: чистая логика — отпечаток до запроса и после ответа совпали (и есть) — его можно класть.
/// Принимает: до, после. Возвращает: отпечаток или None. Ошибок нет.
pub fn same(before: Option<u64>, now: Option<u64>) -> Option<u64> {
    before.filter(|b| Some(*b) == now)
}

/// Ключ памяти: (отпечаток ключа API, монета тикером биржи, сеть биржи).
type Key = (u64, String, String);

/// Память адресов депозита одной биржи. Дешёво клонируется (общий указатель).
pub struct AddrBook<V> {
    map: Arc<RwLock<HashMap<Key, V>>>,
}

impl<V> Clone for AddrBook<V> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

impl<V> Default for AddrBook<V> {
    fn default() -> Self {
        Self {
            map: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl<V: Clone> AddrBook<V> {
    /// Что делает: адрес из памяти под отпечатком (без учёта режима). Принимает: отпечаток (None — нет ключа: ничего),
    /// монету, сеть. Возвращает: адрес или None. Ошибок нет.
    pub fn peek(&self, mark: &Mark, coin: &str, net: &str) -> Option<V> {
        let m = mark.at?;
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(&(m, coin.to_string(), net.to_string())).cloned()
    }

    /// Что делает: решение по режиму задачи для уже найденного в памяти значения (адаптер с отбором, например Kraken —
    /// срок адреса). Принимает: значение из памяти, монету и сеть (для текста). Возвращает: Some — взять из памяти;
    /// None — запросить биржу. Ошибки: NotApplicable — режим «только память», а в памяти нет.
    pub fn apply<T>(&self, mem: Option<T>, coin: &str, net: &str) -> GatewayResult<Option<T>> {
        match decide(mode(), mem) {
            Look::Hit(v) => Ok(Some(v)),
            Look::Ask => Ok(None),
            Look::Miss => Err(GatewayError::new(
                ErrorCode::NotApplicable,
                None,
                format!("Адреса депозита {coin} в сети {net} {MISS}"),
            )),
        }
    }

    /// Что делает: peek + apply. Принимает: отпечаток, монету, сеть. Возвращает и ошибки: как apply.
    pub fn lookup(&self, mark: &Mark, coin: &str, net: &str) -> GatewayResult<Option<V>> {
        let mem = self.peek(mark, coin, net);
        self.apply(mem, coin, net)
    }

    /// Что делает: кладёт адрес в память под отпечатком (заменяет прежний). Отпечатка нет или он сменился за время
    /// запроса (Mark::still) — ничего не делает. Принимает: отпечаток до запроса, монету, сеть, значение. Ошибок нет.
    pub fn keep(&self, mark: &Mark, coin: String, net: String, v: V) {
        if let Some(m) = mark.still() {
            self.put(m, coin, net, v);
        }
    }

    /// Что делает: кладёт под готовым отпечатком. Ошибок нет.
    fn put(&self, m: u64, coin: String, net: String, v: V) {
        {
            let mut map = self.map.write().unwrap_or_else(|p| p.into_inner());
            map.insert((m, coin, net), v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Решение 411: режимы — Normal как раньше, Memory без запроса, Fresh всегда запрос.
    #[test]
    fn decide_by_mode() {
        assert_eq!(decide(Mode::Normal, Some(1)), Look::Hit(1));
        assert_eq!(decide::<i32>(Mode::Normal, None), Look::Ask);
        assert_eq!(decide(Mode::Memory, Some(1)), Look::Hit(1));
        assert_eq!(decide::<i32>(Mode::Memory, None), Look::Miss);
        assert_eq!(decide(Mode::Fresh, Some(1)), Look::Ask);
        assert_eq!(decide::<i32>(Mode::Fresh, None), Look::Ask);
    }

    /// Решение 411: другой отпечаток ключа — адрес старого ключа не находится; без отпечатка — не кладётся.
    #[test]
    fn book_bound_to_mark() {
        let (k1, k2, none) = (
            Mark::fixed(Some(1)),
            Mark::fixed(Some(2)),
            Mark::fixed(None),
        );
        let b: AddrBook<String> = AddrBook::default();
        b.keep(&k1, "ESIM".into(), "BSC".into(), "a1".into());
        assert_eq!(b.peek(&k1, "ESIM", "BSC").as_deref(), Some("a1"));
        assert_eq!(b.peek(&k2, "ESIM", "BSC"), None, "другой ключ API");
        assert_eq!(b.peek(&none, "ESIM", "BSC"), None, "нет ключа");
        b.keep(&none, "LAB".into(), "BSC".into(), "a2".into());
        assert_eq!(b.peek(&k1, "LAB", "BSC"), None);
    }

    /// Решение 411 (ревью п. 2): ключ сменили за время запроса — ответ не кладётся.
    #[test]
    fn keep_only_if_mark_unchanged() {
        assert_eq!(same(Some(1), Some(1)), Some(1));
        assert_eq!(same(Some(1), Some(2)), None);
        assert_eq!(same(Some(1), None), None);
        assert_eq!(same(None, None), None);
    }

    /// Решение 411: режим живёт на задаче и не протекает наружу.
    #[tokio::test]
    async fn mode_scoped() {
        let k1 = Mark::fixed(Some(1));
        assert_eq!(mode(), Mode::Normal);
        assert_eq!(memory_only(async { mode() }).await, Mode::Memory);
        assert_eq!(fresh(async { mode() }).await, Mode::Fresh);
        let b: AddrBook<String> = AddrBook::default();
        let miss = memory_only(async { b.lookup(&k1, "ESIM", "BSC") }).await;
        assert_eq!(miss.unwrap_err().code, ErrorCode::NotApplicable);
        b.keep(&k1, "ESIM".into(), "BSC".into(), "a1".into());
        let f = fresh(async { b.lookup(&k1, "ESIM", "BSC") }).await;
        assert_eq!(f.unwrap(), None, "свежий — память не читается");
        assert_eq!(mode(), Mode::Normal);
    }
}
