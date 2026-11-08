//! Файл: gateway/delist_page.rs
//! Что делает: чтение официальных страниц анонсов делистинга (HTML по url из API анонсов биржи) с кешем по url и
//! лимитом частоты (решение 226, уточнение Тиграна 08.11.2026): страница читается ОДИН раз за запуск (строки текста
//! — в кеше), за одно обновление делистингов биржи читается не больше delist.ann_html_per_refresh новых страниц —
//! остальные в следующий раз. Запрос — публичный GET без cookies с Accept-Language: en-US (английская версия: разбор
//! — по английскому тексту; живая проверка 08.11: без cookies KuCoin, OKX, Bybit, Bitget отдают английскую страницу
//! с текстом анонса в самом HTML). Сбой чтения не кешируется — повтор при следующем обновлении.
//! Кто вызывает: gateway/delist_ann.rs (collect).
//! От чего зависит: gateway/http.rs (exchange, Limits), gateway/delist_html.rs (lines), reqwest.
//! Настройки: delist.ann_html_per_refresh (читает адаптер и передаёт сюда результат), http.* (Limits адаптера).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::gateway::errors::GatewayError;
use crate::gateway::http::{Limits, exchange};

/// Строки уже прочитанных страниц по url (все биржи; url у бирж разные).
static CACHE: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);
/// Язык страницы — английский (разбор времени и пар — по английскому тексту).
const LANG: &str = "en-US";

/// Как читать страницы: HTTP-клиент адаптера, сроки обмена, лимит новых страниц за обновление (результаты чтения
/// настроек — ошибка значит «не задано», страницы тогда не читаются).
pub struct Fetch<'a> {
    pub http: &'a reqwest::Client,
    pub limits: Result<Limits, GatewayError>,
    pub per_refresh: Result<u64, GatewayError>,
}

/// Строки страницы из кеша. None — ещё не прочитана. Ошибок нет.
pub fn cached(url: &str) -> Option<Vec<String>> {
    let g = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    g.as_ref().and_then(|m| m.get(url).cloned())
}

/// Кладёт строки страницы в кеш. Ошибок нет.
fn keep(url: &str, lines: Vec<String>) {
    let mut g = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    g.get_or_insert_with(HashMap::new)
        .insert(url.to_string(), lines);
}

/// Что делает: строки нужных страниц. Принимает: url (без повторов), как читать. Возвращает: (url → строки — из кеша
/// и прочитанные сейчас; url → причина, почему страница не прочитана). Ошибок нет.
pub async fn pages(
    urls: &[String],
    f: &Fetch<'_>,
) -> (HashMap<String, Vec<String>>, HashMap<String, String>) {
    let (mut got, mut missed) = (HashMap::new(), HashMap::new());
    let mut budget = match &f.per_refresh {
        Ok(n) => Ok(*n),
        Err(e) => Err(format!("страницы анонсов не читаются: {}", e.message)),
    };
    for url in urls {
        if let Some(lines) = cached(url) {
            got.insert(url.clone(), lines);
            continue;
        }
        let left = match &mut budget {
            Err(why) => {
                missed.insert(url.clone(), why.clone());
                continue;
            }
            Ok(left) => left,
        };
        if *left == 0 {
            let why = "очередь — за одно обновление читается не больше delist.ann_html_per_refresh страниц";
            missed.insert(url.clone(), why.to_string());
            continue;
        }
        *left -= 1;
        match read(url, f).await {
            Ok(lines) => {
                keep(url, lines.clone());
                got.insert(url.clone(), lines);
            }
            Err(why) => {
                missed.insert(url.clone(), why);
            }
        }
    }
    (got, missed)
}

/// Что делает: читает одну страницу. Принимает: url, как читать. Возвращает: строки текста. Ошибки: текст причины
/// (сроки http.* не заданы, связь, HTTP-код не 2xx, на странице нет текста).
async fn read(url: &str, f: &Fetch<'_>) -> Result<Vec<String>, String> {
    let lim = f.limits.clone().map_err(|e| e.message)?;
    let req = f
        .http
        .get(url)
        .header(reqwest::header::ACCEPT_LANGUAGE, LANG);
    let (status, body) = exchange(req, lim).await.map_err(|e| e.text(&lim))?;
    if !status.is_success() {
        return Err(format!("страница анонса: HTTP {status}"));
    }
    let lines = super::delist_html::lines(&body);
    if lines.is_empty() {
        return Err("страница анонса без текста".into());
    }
    Ok(lines)
}
