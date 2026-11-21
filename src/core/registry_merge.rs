//! Файл: core/registry_merge.rs
//! Что делает: решение 211 (Тигран, 07.11.2026: «метчить монеты только по адресам») — правила записи справочника,
//! собранной по адресу контракта: (1) один тикер на биржу — у одного адреса на ОДНОЙ бирже бывает несколько тикеров
//! (старый и новый токен: «X» и «XOLD» с одним адресом в справочнике сетей); берётся тот, на котором монета на бирже
//! годна (пара торгуется, стакан есть, ввод и вывод открыты); годных больше одного — выбрать нельзя: остаётся одна
//! запись с причиной TickerConflict (монета на этой бирже не торгуется); годных нет — первая по алфавиту со своими
//! причинами (ревью М4: не конфликт); (2) показное имя монеты — тикер
//! большинства бирж монеты, при равенстве — по алфавиту (детерминированно; у DEX своего тикера нет).
//! Кто вызывает: core/registry_build.rs.
//! От чего зависит: registry.rs (Presence, Exclusion).
//! Настройки: нет.

use std::collections::BTreeMap;

use super::registry::{Exclusion, Presence};

/// Одна запись присутствия на биржу. Принимает: присутствия бирж одного адреса (возможно, по несколько на биржу).
/// Возвращает: по одному на биржу, порядок бирж сохранён. Ошибок нет.
pub fn one_per_venue(list: Vec<Presence>) -> Vec<Presence> {
    let mut order: Vec<String> = vec![];
    let mut by_venue: BTreeMap<String, Vec<Presence>> = BTreeMap::new();
    for p in list {
        if !by_venue.contains_key(&p.venue.0) {
            order.push(p.venue.0.clone());
        }
        by_venue.entry(p.venue.0.clone()).or_default().push(p);
    }
    order
        .into_iter()
        .filter_map(|v| by_venue.remove(&v))
        .map(pick)
        .collect()
}

/// Выбор тикера одной биржи. Годен ровно один — он. Годных нет — первый по алфавиту со СВОИМИ причинами (ревью М4:
/// это не конфликт, монета на бирже не торгуется по этим причинам — их и видно). Годных больше одного — конфликт:
/// первый годный с TickerConflict (выбрать нельзя). Ошибок нет.
fn pick(mut same: Vec<Presence>) -> Presence {
    if same.len() == 1 {
        return same.remove(0);
    }
    same.sort_by(|a, b| a.ticker.cmp(&b.ticker));
    let good: Vec<usize> = (0..same.len()).filter(|&i| usable(&same[i])).collect();
    match good.len() {
        // Годных нет — первая по алфавиту, но не токен обёртки, исключённый в пользу нативной (registry_native.rs):
        // запись нативной на бирже не теряется (второе ревью 211, п.2).
        0 => {
            let i = same
                .iter()
                .position(|p| {
                    !p.excluded
                        .iter()
                        .any(|e| matches!(e, Exclusion::NativeVsToken(_)))
                })
                .unwrap_or(0);
            same.remove(i)
        }
        1 => same.swap_remove(good[0]),
        _ => {
            let names: Vec<String> = same.iter().map(|p| p.ticker.clone()).collect();
            let mut first = same.swap_remove(good[0]);
            first
                .excluded
                .push(Exclusion::TickerConflict(names.join(", ")));
            first
        }
    }
}

/// Годна ли запись: причин нет, ввод и вывод открыты. Ошибок нет.
fn usable(p: &Presence) -> bool {
    p.excluded.is_empty() && p.can_send && p.can_receive
}

/// Показное имя монеты: тикер большинства бирж, при равенстве — первый по алфавиту. Нет бирж — пусто. Ошибок нет.
pub fn display_ticker(list: &[Presence]) -> String {
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for p in list {
        *count.entry(p.ticker.as_str()).or_default() += 1;
    }
    let max = count.values().copied().max().unwrap_or(0);
    count
        .into_iter()
        .find(|(_, n)| *n == max)
        .map(|(t, _)| t.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VenueId;

    fn p(v: &str, t: &str, ok: bool) -> Presence {
        Presence {
            venue: VenueId(v.into()),
            ticker: t.into(),
            network: None,
            can_receive: ok,
            can_send: ok,
            can_buy: true,
            can_sell: true,
            excluded: if ok {
                vec![]
            } else {
                vec![Exclusion::NoSpotBook]
            },
            info: None,
            via: None,
        }
    }

    #[test]
    fn one_ticker_per_venue_and_display() {
        // Живой пример 07.11: bingx NMT, gate NETMIND (один адрес); у mexc — тот же адрес под NMT.
        let got = one_per_venue(vec![
            p("bingx", "NMT", true),
            p("gate", "NETMIND", true),
            p("mexc", "NMT", true),
        ]);
        assert_eq!(got.len(), 3);
        assert_eq!(display_ticker(&got), "NMT");
        // На одной бирже два тикера адреса: годен один — он.
        let one = one_per_venue(vec![p("gate", "XOLD", false), p("gate", "X", true)]);
        assert_eq!((one.len(), one[0].ticker.as_str()), (1, "X"));
        // Годны оба — конфликт, монета на бирже не торгуется.
        let both = one_per_venue(vec![p("gate", "B", true), p("gate", "A", true)]);
        assert_eq!(both.len(), 1);
        assert_eq!(
            both[0].excluded,
            vec![Exclusion::TickerConflict("A, B".into())]
        );
        // Годных нет — не конфликт: первая по алфавиту со своими причинами (ревью М4).
        let none = one_per_venue(vec![p("gate", "Y", false), p("gate", "X", false)]);
        assert_eq!(
            (
                none.len(),
                none[0].ticker.as_str(),
                none[0].excluded.clone()
            ),
            (1, "X", vec![Exclusion::NoSpotBook])
        );
        // Равенство голосов — по алфавиту.
        assert_eq!(display_ticker(&[p("a", "Z", true), p("b", "Y", true)]), "Y");
    }
}
