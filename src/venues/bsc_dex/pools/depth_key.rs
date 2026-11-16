//! Файл: venues/bsc_dex/pools/depth_key.rs
//! Что делает: ключ кэша глубины пула (решение 73, pools/depth_book.rs): глубина пересчитывается, если сменилось
//! состояние пула (блок, logIndex последнего учтённого события), настройки глубины (полоса, порог, гистерезис) или
//! курс встречного токена к USDT — состояние пулов «промежуточная — USDT» (пул с BNB, решение 52).
//! Решение 301 (е2): состояние пула и пулов «промежуточная — USDT» — поколение состояния записи (Entry.state_gen,
//! pools/state_gen.rs) вместо (блок, logIndex): перечитанный на новом блоке пул без смены состояния ключ не меняет.
//! Поколение 0 (запись вне реестра) — ключ не совпадает ни с каким, глубина считается заново.
//! Решение 307: Prep отдаёт и сами пулы «промежуточная — USDT» (Rate) — перевод глубины в USDT берёт их отсюда, одним
//! чтением реестра на встречный токен вместо by_pair на каждый to_usdt; поколения в ключе — из тех же записей.
//! Кто вызывает: pools/depth_book.rs.
//! От чего зависит: registry.rs, tokens.rs, ctx.rs.
//! Настройки: нет (значения даёт вызывающий).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::registry::{Entry, Registry};

/// Что посчитано и на чём: поколение состояния пула (решение 301 е2), настройки (полоса, порог, гистерезис) и
/// поколения пулов «промежуточная — USDT», по которым глубина переводится в USDT (пусто — встречный токен USDT).
#[derive(Debug, Clone)]
pub struct Key {
    state_gen: u64,
    band: Decimal,
    min: Decimal,
    hyst: Decimal,
    /// За `Arc` (решение 131): в одном построении графа ключи пулов с одним встречным токеном делят одну копию.
    rate: Arc<Vec<u64>>,
    /// Ревью №1 (P): false — ключ помечен устаревшим (Book::invalidate): не равен никакому.
    fresh: bool,
}

/// Равенство ключей: поколение 0 (запись вне реестра) не равно никакому — такая глубина всегда считается заново.
impl PartialEq for Key {
    fn eq(&self, o: &Key) -> bool {
        self.fresh
            && o.fresh
            && self.state_gen != 0
            && self.state_gen == o.state_gen
            && self.band == o.band
            && self.min == o.min
            && self.hyst == o.hyst
            && self.rate == o.rate
    }
}

impl Key {
    /// Ключ пула `e` при настройках полосы, порога и гистерезиса. Ошибок нет.
    pub fn new(
        ctx: &Ctx,
        reg: &Registry,
        tokens: &Tokens,
        e: &Entry,
        band: Decimal,
        min: Decimal,
        hyst: Decimal,
    ) -> Key {
        Key {
            state_gen: e.state_gen,
            band,
            min,
            hyst,
            rate: Arc::new(rate_key(ctx, reg, tokens, e)),
            fresh: true,
        }
    }

    /// Поколение состояния пула, на котором посчитан ключ (ревью №1 M). Ошибок нет.
    pub fn state_gen(&self) -> u64 {
        self.state_gen
    }

    /// Пометить ключ устаревшим (ревью №1 P, Book::invalidate). Ошибок нет.
    pub fn invalidate(&mut self) {
        self.fresh = false;
    }
}

/// Состояние пулов «промежуточная — USDT» (решение 52), по которым глубина переводится в USDT: их поколения (решение
/// 301 е2; достоверность входит в поколение). Встречный токен — USDT или USDT не известен — пусто. Ошибок нет.
fn rate_key(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
) -> Vec<u64> {
    let Ok(usdt) = tokens.usdt(ctx) else {
        return Vec::new();
    };
    if e.usdt == usdt {
        return Vec::new();
    }
    // Решение 129: отметки без копирования записей пулов (раньше — копия каждого пула «промежуточная — USDT» с картой
    // тиков на каждый пул с промежуточной монетой при каждом построении графа).
    let mut v = reg.pair_marks(e.usdt, usdt);
    v.sort_unstable();
    v
}

/// Поколения пулов пары за `Arc` (решение 301 е2).
type Marks = Arc<Vec<u64>>;

/// Решение 307: пулы «встречный токен X — USDT» на одну подготовку — их поколения (часть ключа глубины) и сами записи
/// (по ним глубина переводится в USDT, quote/rate.rs best_in). Одно чтение реестра (by_pair): поколения в ключе и пулы
/// в расчёте курса согласованы по построению. Дешёво клонируется.
#[derive(Clone)]
pub struct Rate {
    pub marks: Marks,
    pub pools: Arc<Vec<Entry>>,
}

/// Подготовка к расчёту ключей многих пулов подряд (решение 131: построение графа маршрутов считает ключ КАЖДОГО пула
/// реестра): настройки глубины прочитаны один раз, USDT — один раз, пулы «встречный токен — USDT» с их поколениями —
/// один раз на встречный токен, а не на пул (решение 307: и для перевода в USDT, а не by_pair на каждый to_usdt).
/// Ключ — тот же, что у Key::new.
pub struct Prep {
    pub band: Decimal,
    pub min: Decimal,
    pub hyst: Decimal,
    usdt: Option<Address>,
    rates: RefCell<HashMap<Address, Rate>>,
}

impl Prep {
    /// Принимает: окружение, токены, настройки глубины (проверены вызывающим). Ошибок нет.
    pub fn new(ctx: &Ctx, tokens: &Tokens, band: Decimal, min: Decimal, hyst: Decimal) -> Prep {
        Prep {
            band,
            min,
            hyst,
            usdt: tokens.usdt(ctx).ok(),
            rates: RefCell::default(),
        }
    }

    /// Ключ пула `e` (как Key::new). Ошибок нет.
    pub fn key(&self, reg: &Registry, e: &Entry) -> Key {
        let rate = self.rate(reg, e).map(|r| r.marks).unwrap_or_default();
        Key {
            state_gen: e.state_gen,
            band: self.band,
            min: self.min,
            hyst: self.hyst,
            rate,
            fresh: true,
        }
    }

    /// Решение 307: пулы «встречный токен пула — USDT» этой подготовки (берутся один раз на встречный токен).
    /// None — встречный токен и есть USDT или USDT не известен (тогда курс не нужен или считается как раньше).
    /// Ошибок нет.
    pub fn rate(&self, reg: &Registry, e: &Entry) -> Option<Rate> {
        let usdt = self.usdt?;
        if e.usdt == usdt {
            return None;
        }
        let got = self
            .rates
            .borrow_mut()
            .entry(e.usdt)
            .or_insert_with(|| {
                let pools = reg.by_pair(e.usdt, usdt);
                let mut v: Vec<u64> = pools.iter().map(|p| p.state_gen).collect();
                v.sort_unstable();
                Rate {
                    marks: Arc::new(v),
                    pools: Arc::new(pools),
                }
            })
            .clone();
        Some(got)
    }
}
