//! Файл: core/relay_owner.rs
//! Что делает: решение 394 (ревью 4 t179; записка 269, п. 14) — сделка, которой принадлежит перегон USDT, по ID
//! перегона. Перегон выручки сделки идёт со своим ID: «<сделка>u» (шаг закрытия, usdt_return_job.rs), «<сделка>u<n>»
//! (повтор n, usdt_return_retry.rs), «<сделка>m<мс>» («Перегнать», manual_usdt.rs), «<сделка>up<n>» и
//! «<сделка>m<мс>up<n>» (куски общего возврата группы, usdt_return_pieces.rs). Было (факт кода): газ неудачных попыток
//! перегона из кошелька DEX (deal_gas::charge_closed) искал сделку по ID перегона целиком и не находил её — газ
//! писался в журнал «сделки нет», в итог сделки не входил. Теперь: точное совпадение (как раньше), иначе — самый
//! длинный ID сделки, после которого идёт только суффикс одного из видов выше. Ручной перевод USDT (usdt_transfer.rs,
//! «tu<время>») ни одной сделке не принадлежит. Общее для всех направлений и площадок.
//! Кто вызывает: deal_gas.rs (charge_closed); тесты.
//! От чего зависит: ничего (чистая функция по списку ID сделок).
//! Настройки: нет.

/// Что делает: суффикс ID перегона после ID сделки — один из видов шапки: «u», «u<цифры>», «up<цифры>», и любой из них
/// (или пусто) после «m<цифры>». Принимает: суффикс. Возвращает: true — суффикс перегона сделки. Ошибок нет.
fn suffix_ok(s: &str) -> bool {
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    let rest = match s.strip_prefix('m') {
        Some(t) => {
            let end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
            if end == 0 {
                return false;
            }
            let after = &t[end..];
            if after.is_empty() {
                return true;
            }
            after
        }
        None => s,
    };
    match rest.strip_prefix('u') {
        Some("") => true,
        Some(t) => match t.strip_prefix('p') {
            Some(n) => digits(n),
            None => digits(t),
        },
        None => false,
    }
}

/// Что делает: ID сделки, которой принадлежит перегон `id`. Принимает: ID сделок учёта, ID перегона. Возвращает:
/// точное совпадение, иначе самый длинный ID сделки, за которым суффикс перегона (suffix_ok); None — перегон вне
/// сделки. Ошибок нет.
pub fn owner<'a>(deals: impl Iterator<Item = &'a String>, id: &str) -> Option<String> {
    let mut best: Option<&String> = None;
    for k in deals {
        if k == id {
            return Some(k.clone());
        }
        if let Some(s) = id.strip_prefix(k.as_str())
            && suffix_ok(s)
            && best.is_none_or(|b| k.len() > b.len())
        {
            best = Some(k);
        }
    }
    best.cloned()
}
