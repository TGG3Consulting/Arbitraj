//! Файл: core/credit_rest.rs
//! Что делает: решение 413 (пятое – седьмое ревью t198) — остаток «не присвоено» (CreditFact.rest) не теряется и не
//! становится фантомом. По решению 76 счёт биржи — только бота: всё, что на нём сверх учёта сделок, — монеты сделок
//! бота. Остаток решается только в ТИХИЙ МОМЕНТ счёта (биржа, монета): нет других ждущих доли, сделок в пути сюда /
//! уходящих отсюда, продаж с неизвестным исходом и сделок с остатком, которые сейчас в работе. Остатки других сделок,
//! что не в работе (остановлены, ждут человека, ждут тихий момент после продажи), — не помеха, а участники
//! СОВМЕСТНОГО решения (седьмое ревью, Б-1): пул = факт − подтверждённые всех прочих (и «Беру на себя»); пул делится
//! между всеми сделками с остатком: сначала их подтверждённые количества (не хватает — поровну по доле количества),
//! затем добавка пропорционально остаткам, каждой не больше «количество + остаток» (share_out). Одно чтение на счёт,
//! под одним замком (credit_rest_lock.rs).
//! Тексты: меньше количества — Warn и Telegram («не хватает»); больше ожидаемого по удержанию — «удержание биржи меньше
//! ожидаемого»; больше записи депозита — «взято сверх зачисления N — остатки прошлых сделок (решение 76)» (Info).
//! Ожидание после продажи, отдельная позиция и круг сверки — credit_rest_park.rs.
//! Кто вызывает: core/credit_rest_lock.rs, tests.
//! От чего зависит: credit_fact.rs, credit_lock.rs (unsure), remain_others.rs, deal_words.rs.
//! Настройки: нет.

use std::collections::HashSet;

use rust_decimal::Decimal;

use super::deal::Deal;
use super::deal_words as w;
use super::engine::Core;
use super::remain_others::Others;

pub use super::credit_rest_lock::{Locked, Res, for_button, locked};

/// Что делает: почему момент счёта не тихий (шапка); None — тихий (остатки других, что не в работе, — не помеха).
/// В тексте — только настоящие помехи (восьмое ревью). Принимает: ядро, сделку, учёт других. Ошибок нет.
pub fn busy(core: &Core, deal: &Deal, o: &Others) -> Option<String> {
    let idle: HashSet<&str> = o
        .rests
        .iter()
        .filter(|r| r.3)
        .map(|r| r.0.as_str())
        .collect();
    let unclear: Vec<&str> = o
        .unclear
        .iter()
        .map(String::as_str)
        .filter(|id| !idle.contains(id))
        .collect();
    let unsure = super::credit_lock::unsure(core, &deal.at, &deal.direction.asset, &deal.id);
    let pending: Vec<&str> = o.pending.iter().map(|p| p.id.as_str()).collect();
    let mut why = Vec::new();
    if !pending.is_empty() {
        why.push(format!("ждут доли сделки {}", pending.join(", ")));
    }
    if !unclear.is_empty() {
        why.push(format!(
            "в пути сюда, остаток в работе или исход продажи неизвестен — сделки {}",
            unclear.join(", ")
        ));
    }
    if !o.leaving.is_empty() {
        why.push(format!(
            "уходят с этого счёта сделки {}",
            o.leaving.join(", ")
        ));
    }
    if unsure {
        why.push("продажа другой сделки с неизвестным исходом".into());
    }
    (!why.is_empty()).then(|| why.join("; "))
}

/// Что делает: дележ пула между сделками с остатком (шапка; восьмое ревью С-1). Первая — своя сделка, остальные —
/// соучастники. Подтверждённые количества соучастников не урезаются никогда: им — их количество; своей — из того, что
/// осталось, не больше её количества (своя с неизвестным исходом продажи передаёт количество с базой 0 — всё её в
/// остатке); остаток пула — добавкой пропорционально остаткам, каждой не больше «количество + остаток».
/// Принимает: пул, (база, остаток) каждой (своя — первой), точность. Возвращает: новые количества по порядку. Ошибок нет.
pub fn share_out(pool: Decimal, parts: &[(Decimal, Decimal)], scale: u32) -> Vec<Decimal> {
    let Some(((own, own_rest), peers)) = parts.split_first() else {
        return Vec::new();
    };
    let peers_base: Decimal = peers.iter().map(|p| p.0).sum();
    let left = (pool - peers_base).max(Decimal::ZERO);
    let own_base = (*own).min(left);
    let mut out = vec![own_base];
    out.extend(peers.iter().map(|p| p.0));
    let extra = left - own_base;
    let rests: Decimal = own_rest + peers.iter().map(|p| p.1).sum::<Decimal>();
    if extra > Decimal::ZERO && rests > Decimal::ZERO && own_base == *own {
        let add = extra.min(rests);
        for (q, p) in out.iter_mut().zip(parts) {
            *q += p.1 * add / rests;
        }
    }
    out.into_iter()
        .zip(parts)
        .map(|(q, p)| q.min(p.0 + p.1).trunc_with_scale(scale))
        .collect()
}

/// Итог решения остатка одной сделки: тексты для журнала.
#[derive(Default)]
pub struct Resolved {
    /// Вышло меньше количества — Warn и Telegram.
    pub short: Option<String>,
    /// Больше ожидаемого по удержанию или больше записи — Info.
    pub over: Option<String>,
}

/// Что делает: записывает решённое количество сделке (остатка больше нет, отметка «исход неизвестен» снята) и тексты.
/// Принимает: сделку, новое количество, факт, подтверждённые прочих, ожидаемое удержание %, доли соучастников
/// словами (пусто — нет). Возвращает: Resolved.
/// Ошибок нет.
pub fn apply_one(
    deal: &mut Deal,
    qty: Decimal,
    fact: Decimal,
    others: Decimal,
    hold: Decimal,
    peers: &str,
) -> Resolved {
    let Some(rest) = super::credit_fact::rest(deal) else {
        return Resolved::default();
    };
    let t = deal.direction.asset.ticker.clone();
    let before = deal.qty;
    super::credit_fact::confirm(deal, qty, super::credit_fact::Basis::Balance);
    deal.sale_unsure = false;
    let mut text = format!(
        "Остаток зачисления решён в тихий момент счёта (решение 413): было {}, не присвоено {}, на счёте {}, у других \
         {} — у сделки {}",
        w::coins(before, &t),
        w::coins(rest, &t),
        w::coins(fact, &t),
        w::coins(others, &t),
        w::coins(qty, &t)
    );
    if !peers.is_empty() {
        text.push_str(&format!("; совместно с {peers}"));
    }
    let record = super::credit_fact::here(deal).map_or(Decimal::ZERO, |c| c.record);
    let sold: Decimal = deal.sells.iter().map(|s| s.filled_qty).sum();
    let expected = record * (Decimal::ONE - hold / Decimal::ONE_HUNDRED);
    let mut over = None;
    if qty + sold > record {
        let n = w::coins(qty + sold - record, &t);
        text.push_str(&format!(
            "; взято сверх зачисления {n} — остатки прошлых сделок (решение 76)"
        ));
        over = Some(format!("{}: {text}", deal.id));
    } else if qty + sold > expected {
        let n = w::coins(qty + sold - expected, &t);
        text.push_str(&format!(
            "; больше ожидаемого на {n} — удержание биржи меньше ожидаемого"
        ));
        over = Some(format!("{}: {text}", deal.id));
    }
    deal.note(text.clone());
    let short = (qty < before).then(|| {
        format!(
            "{}: {text}; не хватает {}",
            deal.id,
            w::coins(before - qty, &t)
        )
    });
    Resolved { short, over }
}
