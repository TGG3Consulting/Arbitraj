//! Файл: core/feed_print.rs
//! Что делает: решение 213, подтверждение тихой пары ПО СОДЕРЖИМОМУ стакана (Тигран 08.11: «запросили стакан по REST,
//! сравнили с нашими данными; совпал — пара тихая, торгуем; не совпал — кадры потеряны, переподписка пары»). Для бирж,
//! у которых документация не связывает номер кадра WebSocket с номером ответа REST (MEXC, Bitget — решения
//! «202/213 (MEXC)», «202/213 (Bitget)»). Правило без порядка: первые N уровней обеих сторон ответа REST и стакана в
//! памяти адаптера равны (точно, по десятичным значениям) — Quiet; не равны или стакана в памяти нет — Lost.
//! Гонка «книга изменилась между кадром и ответом REST» разбирается в feed_confirm.rs::apply: свежий кадр за время
//! запроса — ответ не нужен (сравнение со свежим снимком feed_health); иначе худший случай — лишняя переподписка.
//! Кто вызывает: core/feed_confirm.rs (verdict).
//! От чего зависит: gateway/feed_health.rs (BookPrints), feed_confirm.rs (Verdict).
//! Настройки: нет (N задаёт адаптер — venue.depth_levels площадки).

use crate::gateway::feed_health::{BookPrint, BookPrints};

use super::feed_confirm::Verdict;

/// Сравнение по содержимому. Принимает: отпечатки REST и своего стакана. Возвращает: Quiet — равны; Lost с текстом —
/// не равны (лучшие уровни обеих сторон в тексте) или стакана пары в памяти нет. Ошибок нет.
pub fn by_content(p: &BookPrints) -> Verdict {
    match &p.ours {
        Some(ours) if *ours == p.rest => Verdict::Quiet,
        Some(ours) => Verdict::Lost(format!(
            "первые {} уровней стакана на бирже (REST) не совпадают со стаканом в памяти: биржа {}, у нас {}",
            p.levels,
            best(&p.rest),
            best(ours)
        )),
        None => Verdict::Lost(format!(
            "на бирже книга есть ({}), а стакана пары в памяти нет",
            best(&p.rest)
        )),
    }
}

/// Лучшие уровни для журнала: «bid цена×объём / ask цена×объём, уровней b/a». Ошибок нет.
fn best(b: &BookPrint) -> String {
    let lv = |l: Option<&crate::core::types::PriceLevel>| {
        l.map(|l| format!("{}×{}", l.price, l.qty))
            .unwrap_or_else(|| "—".into())
    };
    format!(
        "bid {} / ask {}, уровней {}/{}",
        lv(b.bids.first()),
        lv(b.asks.first()),
        b.bids.len(),
        b.asks.len()
    )
}
