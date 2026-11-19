//! Файл: core/remain_others.rs
//! Что делает: решение 405 (t191, Тигран 18.11: «каждая сделка всегда должна знать своё количество монет и работать
//! только с ним»; ревью Н1′ / Н7) — сколько монет на счёте площадки ДОКАЗУЕМО числится за другими сделками той же
//! монеты, чтобы сверка остатка (remain_fact.rs, решение 222) не отдала сделке чужие монеты на общем счёте / кошельке:
//! остаток = min(учёт сделки, факт счёта − учёт других сделок на этом счёте).
//!
//! Другие сделки (по набору unsettled.rs — без прохода по истории): та же монета, не эта сделка, монет > 0
//! (Deal.qty), шаг не «закрыта» и не «Беру на себя» (ревью 3, Н1: монеты у человека — balance_sell_check.rs их не
//! считает ботовыми; manual.rs qty не обнуляет), счёт тот же (биржа — один на все сделки площадки, remain_fact.rs fact;
//! DEX — тот же кошелёк Deal.dex_account, неизвестен — тот же). Остановленные (Halted) и ждущие человека — считаются
//! (монеты бота).
//!
//! Где монеты другой сделки (ревью 3, Б1):
//! - шаг не «перевод» — на Deal.at (после покупки — площадка покупки, после зачисления — получатель: fills.rs
//!   transfer ставит at и qty = «сколько дошло», комиссия вывода в монете уже вычтена — ревью 3, М3);
//! - шаг «перевод» (Transfer) — не доказано: шаг ставится ДО запроса вывода (executor.rs, relay.rs, manual_relay.rs),
//!   признака «вывод запрошен» в сделке нет, at и qty меняются только после зачисления (qty в пути — купленное, до
//!   комиссии вывода). Такая сделка на площадке покупки или получателя этого счёта — «неясно» (Others::unclear).
//!
//! «Неясные» (в пути) не вычитаются — их монеты могли уйти или ещё не прийти; доказуемые другие вычитаются всегда
//! (ревью 4, Н1: прежде «неясная» отключала вычет всех): остаток = min(учёт, факт − доказуемых). Пример: Y учёт 1000
//! (фантом), Z продаёт 5000 доказуемо на W, K в пути на W, на W 5000 → своих у Y нет — человеку, а не 1000; на W 5600 →
//! Y ведёт 600. В журнал — и доказуемые, и сделки в пути.
//! Факт меньше учёта доказуемых других — монеты на счёте по учёту чужие: сделке не достаётся ничего, но она НЕ
//! закрывается как фантом (у кого фантом — по факту не видно), решает человек (remain_fact.rs all_foreign).
//! Решение 413 (t198, живой WKC 18.11): другая сделка, чьё зачисление на эту биржу ещё не подтверждено фактом («ждёт
//! доли», credit_fact.rs), — не в qty (её учёт = запись депозита, а на счёте может быть меньше: удержание биржи), а в
//! pending: её долю считает распределение факта (credit_split.rs) вместе со своей — сделки не видят друг у друга «своих
//! монет нет» из-за удержания биржи (было: вторая сделка WKC вычитала запись первой 6 008 053 740.36 из факта
//! 5 915 283 179 — «всё чужое»).
//! Кто вызывает: remain_fact.rs (reconcile), credit_base.rs, credit_apply.rs.
//! От чего зависит: unsettled.rs, coin_key.rs, deal.rs, trade_ops.rs (вид площадки), engine.rs, credit_fact.rs.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Монеты других сделок на счёте: сумма доказуемо лежащих, (сделка, монет), сделки в пути через этот счёт.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Others {
    pub qty: Decimal,
    pub deals: Vec<(String, Decimal)>,
    /// Сделки на шаге «перевод» с площадкой покупки или получателем = эта площадка (где их монеты — не доказано).
    pub unclear: Vec<String>,
    /// Решение 413: другие сделки на этом счёте, ждущие доли (запись депозита и посторонние до вывода) — не в qty.
    pub pending: Vec<super::credit_split::Pending>,
    /// Решение 413 (повторное ревью, С-1): сделки на шаге «перевод», уходящие с этого счёта (вывод отсюда, получатель —
    /// другой): не «в пути сюда», не вычитаются (решение 405, ревью 3 Б1 — их монеты здесь или уже нет, не доказано),
    /// но замер удержания и снимок посторонних при них неоднозначны.
    pub leaving: Vec<String>,
    /// Третье ревью (С-Е): их количество — пул ждущих долю его не включает (монеты могут ещё лежать здесь).
    pub leaving_qty: Decimal,
    /// Седьмое ревью t198 (Б-1): другие сделки с остатком «не присвоено» (они же в unclear): (ID, количество, остаток,
    /// не в работе — остановлена, ждёт человека или ждёт тихий момент после продажи с количеством 0). Совместное решение
    /// остатков — credit_rest.rs.
    pub rests: Vec<(String, Decimal, Decimal, bool)>,
    /// Те же сделки словами для тревоги (deal_words.rs): доказуемые, затем в пути (требование Тиграна 18.11).
    pub words: Vec<String>,
}

/// Что делает: монеты других сделок той же монеты на счёте сделки `deal` на площадке `v` (см. шапку). Принимает: ядро,
/// сделку, площадку. Возвращает: Others. Ошибок нет. Замок: deals (по набору unsettled).
pub fn on_account(core: &Core, deal: &Deal, v: &VenueId) -> Others {
    let cex = super::trade_ops::kind(core, v) == Ok(VenueKind::Cex);
    let key = CoinKey::of(&deal.direction.asset);
    let ids = super::unsettled::ids(core);
    let deals = lk(&core.inner.deals);
    let mut out = Others::default();
    for d in ids.iter().filter_map(|id| deals.get(id)) {
        // Шестое ревью (Б-2): сделка с остатком «не присвоено» учитывается при любом количестве (в ожидании после
        // продажи её количество 0).
        if d.id == deal.id
            || (d.qty <= Decimal::ZERO && super::credit_fact::rest(d).is_none())
            || matches!(d.step, Step::Closed | Step::TakenOver)
            || CoinKey::of(&d.direction.asset) != key
        {
            continue;
        }
        let same = cex
            || d.dex_account.is_none()
            || deal.dex_account.is_none()
            || d.dex_account == deal.dex_account;
        if !same {
            continue;
        }
        // Шестое ревью (Б-2): у сделки остаток «не присвоено» — её подтверждённое количество вычитается, а остаток —
        // «неясно»: не вычитается и никому не выдаётся.
        if d.at == *v
            && let Some(r) = super::credit_fact::rest(d)
        {
            out.qty += d.qty;
            out.deals.push((d.id.clone(), d.qty));
            out.unclear.push(d.id.clone());
            let idle =
                matches!(d.step, Step::Halted | Step::WaitingHuman) || d.qty <= Decimal::ZERO;
            out.rests.push((d.id.clone(), d.qty, r, idle));
            out.words.push(format!(
                "{} — у неё не присвоено {}",
                super::deal_words::deal(core, d),
                super::deal_words::coins(r, &d.direction.asset.ticker)
            ));
            continue;
        }
        // Пятое ревью t198 (С-7): продажа сделки с неизвестным исходом — её монеты на счёте не доказаны (в том числе у
        // остановленной): не «подтверждённые», а «неясно».
        if d.sale_unsure && d.at == *v {
            out.unclear.push(d.id.clone());
            out.words.push(format!(
                "{} — исход её продажи неизвестен, сколько её монет на счёте, не доказано",
                super::deal_words::deal(core, d)
            ));
            continue;
        }
        if d.step == Step::Transfer {
            // Повторное ревью t198 (С-1): «в пути через счёт» — только сделка, у которой ПОЛУЧАТЕЛЬ — этот счёт
            // (Deal.transit_to; у записей без него — площадка продажи, если монеты не здесь). Уходящие с этого счёта
            // (площадка покупки, перегон отсюда) — не «в пути сюда»: их монет здесь уже или ещё нет по учёту.
            let to_here = match &d.transit_to {
                Some(t) => t == v,
                None => d.direction.sell_on == *v && d.at != *v,
            };
            if to_here {
                out.unclear.push(d.id.clone());
                out.words.push(format!(
                    "{} — в пути, где монеты сейчас, не доказано",
                    super::deal_words::deal(core, d)
                ));
            } else if d.at == *v {
                out.leaving.push(d.id.clone());
                out.leaving_qty += d.qty;
            }
        } else if d.at == *v
            && super::credit_fact::pending(d)
            && let Some(c) = super::credit_fact::here(d)
        {
            out.pending.push(super::credit_split::Pending {
                id: d.id.clone(),
                record: c.record,
                outsider: c.outsider,
                seen: c.outsider_at,
            });
            out.words.push(format!(
                "{} — зачисление ещё не подтверждено фактом (запись депозита {})",
                super::deal_words::deal(core, d),
                super::deal_words::coins(c.record, &d.direction.asset.ticker)
            ));
        } else if d.at == *v {
            out.qty += d.qty;
            out.deals.push((d.id.clone(), d.qty));
            out.words.push(super::deal_words::deal(core, d));
        }
    }
    drop(deals);
    // Шестое ревью (Б-2): монеты «Беру на себя» на этом счёте — человека: вычитаются, как подтверждённые.
    for (h, d) in lk(&core.inner.holdings).values() {
        if h.venue == *v && h.qty > Decimal::ZERO && CoinKey::of(&h.asset) == key && d.id != deal.id
        {
            out.qty += h.qty;
            out.deals.push((d.id.clone(), h.qty));
            out.words.push(format!(
                "{} — у человека («Беру на себя»)",
                super::deal_words::coins(h.qty, &h.asset.ticker)
            ));
        }
    }
    out.deals.sort();
    out.unclear.sort();
    out.rests.sort_by(|x, y| x.0.cmp(&y.0));
    out.leaving.sort();
    out.pending.sort_by(|x, y| x.id.cmp(&y.id));
    out
}

/// Итог сверки остатка.
#[derive(Debug, Clone, PartialEq)]
pub enum Own {
    /// Вести столько (0 — на счёте ничего нет: закрыть, как раньше).
    Qty(Decimal),
    /// На счёте что-то есть, но по учёту всё чужое: не закрывать, решает человек.
    Human,
}

/// Что делает: сколько вести сделке (ревью 4, Н1: вычитаются доказуемые другие, «неясные» — в пути — не вычитаются и
/// ничего не отключают). Принимает: учёт сделки, факт счёта, других (Others). Возвращает: факт ≤ 0 — Qty(0) (на счёте
/// пусто — закрыть, как раньше); факт − доказуемых других ≤ 0 — Human (на счёте по учёту только чужое: не закрывать,
/// не продавать — человеку; и при «неясных»: закрыть как фантом нельзя, а вести учёт — значит продать чужие); иначе
/// Qty(min(учёт, факт − доказуемых)). Других нет — то же, что remain_fact::settle_qty. Ошибок нет.
pub fn own_qty(own: Decimal, fact: Decimal, o: &Others) -> Own {
    if fact <= Decimal::ZERO {
        return Own::Qty(Decimal::ZERO);
    }
    let free = fact - o.qty;
    if free <= Decimal::ZERO {
        return Own::Human;
    }
    Own::Qty(own.min(free).max(Decimal::ZERO))
}
