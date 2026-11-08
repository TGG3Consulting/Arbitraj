//! Файл: gateway/delist_html.rs
//! Что делает: строгий разбор официальной страницы анонса делистинга (HTML по url из API анонсов) — только когда API
//! анонсов не отдаёт тикеры или время (решение 226, уточнение Тиграна 08.11.2026: «непокрытого быть не должно»).
//! (1) HTML → строки текста по блокам (p, li, br, h*, div; строка таблицы tr — ячейки через « | »), скрипты, стили и
//!     ссылки на другие анонсы (блок «похожие статьи» — живая страница Bitget 08.11: «… WIN/USDT …») выбрасываются; (2) пары: слово страницы = пара справочника биржи с котируемой монетой venue.quote_asset
//!     («HFTUSDT», «ALPINE/USDT», «DORA-USDT», «X_USDT») или строка вида «Название (ТИКЕР)» (список KuCoin) с тикером
//!     из справочника — других тикеров не бывает; (3) время делистинга пары: в строках про торги (без слов про ботов,
//!     депозит, вывод, конвертацию, Earn, копи-трейдинг, маржу, займы, фьючерсы) — строка с этой парой и временем
//!     (таблица OKX: у каждой пары своё), иначе единственное время всех строк про делистинг/торги; несколько разных —
//!     время не определено (монета блокируется без времени).
//! Живые страницы 08.11.2026 — tests/data/delist_ann_2026-11-08/ (KuCoin, Bybit, OKX, Bitget).
//! Кто вызывает: gateway/delist_page.rs (lines — при чтении страницы), gateway/delist_ann.rs (pairs), tests.
//! От чего зависит: gateway/delist_time.rs, chrono.
//! Настройки: нет — это грамматика страниц анонсов.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use super::delist_time;

/// Теги, после которых начинается новая строка текста.
const BLOCK: [&str; 13] = [
    "p", "li", "br", "div", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol",
];
/// Пути ссылок на страницы анонсов (Bitget /support/articles/, KuCoin /announcement/, Bybit /article/, OKX /help/).
const LINKS: [&str; 4] = [
    "/support/articles/",
    "/announcement/",
    "/article/",
    "/help/",
];
/// Слова строк не про спотовые торги (время из них не берётся, пары не ищутся).
const OTHER: [&str; 21] = [
    "bot",
    "bots",
    "deposit",
    "deposits",
    "withdraw",
    "withdrawal",
    "withdrawals",
    "withdraws",
    "convert",
    "earn",
    "copy",
    "margin",
    "loan",
    "loans",
    "lending",
    "collateral",
    "futures",
    "perpetual",
    "contract",
    "contracts",
    "options",
];

/// Что делает: строки текста страницы. Принимает: HTML. Возвращает: непустые строки (сущности &amp; &nbsp; &lt;
/// &gt; &quot; &#39; раскрыты, пробелы сжаты). Ошибок нет.
pub fn lines(html: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_row = false;
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        cur.push_str(&rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else {
            break;
        };
        let raw = &rest[lt + 1..lt + gt];
        let open = !raw.starts_with('/') && !raw.ends_with('/');
        let name: String = raw
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        rest = &rest[lt + gt + 1..];
        // Ссылка на ДРУГОЙ анонс (блок «похожие статьи» рядом с текстом: Bitget «Notice of Delisting 5 Spot Trading
        // Pairs…», «… WIN/USDT …») — не текст этого анонса, выбрасывается целиком.
        let other_ann = name == "a" && LINKS.iter().any(|m| raw.contains(m));
        if name == "script" || name == "style" || other_ann {
            if open {
                let close = format!("</{name}");
                let end = rest.to_ascii_lowercase().find(&close).unwrap_or(rest.len());
                rest = &rest[end..];
            }
            continue;
        }
        if name == "tr" {
            // Строка таблицы — одна строка текста, даже если в ячейках абзацы (OKX: <td><p>DORA/USDT</p></td>).
            push(&mut out, &mut cur);
            in_row = open;
        } else if name == "td" || name == "th" {
            if open {
                cur.push_str(" | ");
            }
        } else if !in_row && BLOCK.contains(&name.as_str()) {
            push(&mut out, &mut cur);
        } else if in_row {
            cur.push(' ');
        }
    }
    cur.push_str(rest);
    push(&mut out, &mut cur);
    out
}

/// Дописывает строку (с раскрытыми сущностями и сжатыми пробелами), если она не пуста. Ошибок нет.
fn push(out: &mut Vec<String>, cur: &mut String) {
    let raw = std::mem::take(cur)
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = line
        .trim_matches(|c: char| c == '|' || c.is_whitespace())
        .to_string();
    if !line.is_empty() {
        out.push(line);
    }
}

/// Что делает: строка про спотовые торги — в ней нет слов про ботов, депозит, вывод, Convert, Earn, копи-трейдинг,
/// маржу, займы, фьючерсы (адаптеры так же отсеивают заголовки анонсов: «… Perpetual Contract», «… as Collateral
/// and Lending Asset» не спот). Принимает: строку. Возвращает: true — про спот. Ошибок нет.
pub fn spot(line: &str) -> bool {
    !line
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|w| OTHER.contains(&w.to_ascii_lowercase().as_str()))
}

/// Что делает: тикеры справочника в строке. Принимает: строку, справочник «тикер → символ», котируемую монету.
/// Возвращает: тикеры (без повторов, по порядку). Ошибок нет.
fn found(line: &str, known: &HashMap<String, String>, quote: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut add = |t: &str| {
        if known.contains_key(t) && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    };
    for w in line.split(|c: char| !(c.is_ascii_alphanumeric() || "/-_".contains(c))) {
        if let Some((b, q)) = w.split_once(['/', '-', '_']) {
            if q == quote {
                add(b);
            }
        } else if let Some(b) = w.strip_suffix(quote).filter(|b| !b.is_empty()) {
            add(b);
        }
    }
    // Список KuCoin: «Ice Open Network (ION)» — вся строка, тикер в скобках в конце, времени в строке нет.
    if let Some(inner) = line.strip_suffix(')').and_then(|l| l.rsplit_once(" (")) {
        let t = inner.1;
        let ok = t
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if ok && delist_time::all(line).is_empty() {
            add(t);
        }
    }
    out
}

/// Строка про делистинг или торги (для общего времени страницы). Ошибок нет.
fn about(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("delist") || l.contains("trading")
}

/// Что делает: пары страницы и время делистинга каждой. Принимает: строки страницы (lines), справочник «тикер →
/// символ» с котируемой монетой quote, саму quote. Возвращает: тикер → время (None — время не определено однозначно),
/// по алфавиту. Пусто — пар справочника на странице нет. Ошибок нет.
pub fn pairs(
    lines: &[String],
    known: &HashMap<String, String>,
    quote: &str,
) -> BTreeMap<String, Option<DateTime<Utc>>> {
    let kept: Vec<&String> = lines.iter().filter(|l| spot(l)).collect();
    let common = delist_time::distinct(
        kept.iter()
            .filter(|l| about(l))
            .flat_map(|l| delist_time::all(l))
            .collect(),
    );
    let mut own: BTreeMap<String, Vec<DateTime<Utc>>> = BTreeMap::new();
    for l in &kept {
        let times = delist_time::all(l);
        for t in found(l, known, quote) {
            own.entry(t).or_default().extend(times.iter().copied());
        }
    }
    own.into_iter()
        .map(|(t, times)| {
            let at = if times.is_empty() {
                common
            } else {
                delist_time::distinct(times)
            };
            (t, at)
        })
        .collect()
}
