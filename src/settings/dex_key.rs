//! Файл: settings/dex_key.rs
//! Что делает: решение 490 — суффикс сети ключа области «DEX» (решение 435/433-з) одним правилом для всех, кто его
//! знает, и проверка ключа «id@сеть», который вводит человек («Добавить DEX» на экране «Настройки»).
//! Правило суффикса (раньше жило только в venues/bsc_dex/ctx_place.rs): площадка DEX в процессе одна — её сеть по
//! умолчанию, суффикса нет, настройки не читаются; несколько — dex.chain площадки равна dex.default_chain → суффикса
//! нет (ключ = id, как все живые настройки BSC), иначе «@» + dex.chain. Площадка (ctx_place dex_suffix), интерфейс
//! (GET /api/venues/{id}/dex-key-suffix, api/dex_key.rs) и проверка ключа (check_net) берут его отсюда — правило одно.
//! Проверка ключа (check_net): «id@сеть» допустим, только если в шлюзе есть площадка DEX, у которой суффикс ровно
//! «@сеть» (её сеть — не сеть по умолчанию). Иначе причина: сеть — сеть по умолчанию (ключ без «@») или площадки DEX
//! этой сети нет. Ключ без «@» и уже существующий ключ (есть хоть одно значение) не проверяются — как
//! check_new_dex_id_len: живые настройки не запираются. Грамматику ключа проверяет scope.rs check_dex_id.
//! Кто вызывает: venues/bsc_dex/ctx_place.rs (suffix), api/dex_key.rs (venue_suffix — эндпоинт суффикса),
//! api/settings.rs (check_net — PUT /api/settings), тесты tests/settings_dex_key_490.rs.
//! От чего зависит: settings/store.rs, settings/read.rs (At), settings/scope.rs, gateway/errors.rs.
//! Настройки: dex.chain [площадка DEX], dex.default_chain [вся система] — только читает.

use crate::gateway::errors::GatewayError;

use super::read::At;
use super::scope::Scope;
use super::store::Store;

/// Что делает: суффикс ключа области «DEX» площадки (правило — шапка файла).
/// Принимает: одна ли площадка DEX в процессе; чтение dex.chain площадки; чтение dex.default_chain (вызываются, только
/// когда площадок несколько, в этом порядке). Возвращает: None — сеть по умолчанию; Some(«@сеть») — другая сеть.
/// Ошибки: ошибка чтения (не задана dex.chain или dex.default_chain) — как вернул читатель.
pub fn suffix<E>(
    alone: bool,
    own: impl FnOnce() -> Result<String, E>,
    default: impl FnOnce() -> Result<String, E>,
) -> Result<Option<String>, E> {
    if alone {
        return Ok(None);
    }
    let own = own()?;
    let def = default()?;
    if own.trim() == def.trim() {
        return Ok(None);
    }
    Ok(Some(format!("@{}", own.trim())))
}

/// Что делает: суффикс ключа области «DEX» площадки по хранилищу настроек — те же чтения, что у окружения адаптера
/// (dex.chain площадки с подстановкой «площадка → система», dex.default_chain «вся система»).
/// Принимает: хранилище, id площадки DEX, одна ли площадка DEX в шлюзе. Возвращает: как suffix.
/// Ошибки: NotConfigured — при нескольких площадках не задана dex.chain или dex.default_chain; Internal — неверный вид.
pub fn venue_suffix(s: &Store, venue: &str, alone: bool) -> Result<Option<String>, GatewayError> {
    let at = At {
        venue: Some(venue),
        coin: None,
    };
    suffix(
        alone,
        || s.text("dex.chain", at),
        || s.text("dex.default_chain", At::default()),
    )
}

/// Что делает: проверка нового ключа «id@сеть» области «DEX» перед записью (правило — шапка файла).
/// Принимает: хранилище, ключ (уже прошёл check_dex_id), id площадок DEX шлюза.
/// Возвращает: Ok(()) — ключ без «@», уже существует или есть площадка DEX этой сети не по умолчанию.
/// Ошибки: текст причины — сеть по умолчанию (ключ без «@») или нет площадки DEX этой сети (с причинами площадок, у
/// которых суффикс не определить).
pub fn check_net(s: &Store, key: &str, dexes: &[String]) -> Result<(), String> {
    let Some((_, net)) = key.split_once('@') else {
        return Ok(());
    };
    let scope = Scope::Dex(key.to_string());
    if s.all().iter().any(|(_, sc, _)| *sc == scope) {
        return Ok(());
    }
    let want = format!("@{net}");
    let alone = dexes.len() <= 1;
    let mut why: Vec<String> = Vec::new();
    for v in dexes {
        match venue_suffix(s, v, alone) {
            Ok(Some(sfx)) if sfx == want => return Ok(()),
            Ok(None) if chain_of(s, v).as_deref() == Some(net) => {
                return Err(format!(
                    "Ключ «{key}»: сеть «{net}» у площадки {v} — сеть по умолчанию, ключ области «DEX» без «@» \
                     (просто id DEX)"
                ));
            }
            Ok(_) => {}
            Err(e) => why.push(format!("{v}: {}", e.message)),
        }
    }
    let list = if dexes.is_empty() {
        "площадок DEX в шлюзе нет".to_string()
    } else {
        format!("площадки DEX: {}", dexes.join(", "))
    };
    let extra = if why.is_empty() {
        String::new()
    } else {
        format!("; сеть не определить — {}", why.join("; "))
    };
    Err(format!(
        "Ключ «{key}»: нет площадки DEX сети «{net}» ({list}){extra}"
    ))
}

/// dex.chain площадки без пробелов по краям; None — не задана. Ошибок нет.
fn chain_of(s: &Store, venue: &str) -> Option<String> {
    let at = At {
        venue: Some(venue),
        coin: None,
    };
    s.text("dex.chain", at).ok().map(|c| c.trim().to_string())
}
