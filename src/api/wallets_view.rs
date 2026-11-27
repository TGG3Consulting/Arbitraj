//! Файл: api/wallets_view.rs
//! Что делает: данные экрана «Маппинг кошельков» (решение 132): кошельки (id, имя, имя секрета ключа, адрес — от
//! адаптера, исполнитель, запрет развёртывания исполнителя — решение 399, классы шага v3 и их приёмка — решение 400,
//! со сводкой пробы пулов через исполнитель этого кошелька — решение 401,
//! строки маппинга, незавершённые сделки),
//! строки маппинга (каждая биржа реестра шлюза — без перечисления в коде, «DEX ↔ DEX», «Пробы и сигналы») и
//! предупреждения «деньги смешиваются». Ключей нет никогда.
//! Кто вызывает: api/wallets.rs (list, map).
//! От чего зависит: api/{state,wallets_dex}.rs, gateway (реестр), settings/accounts/, core/dex_account.rs.
//! Решение 432: у кошелька — chains (исполнитель на площадке DEX каждой сети; одна площадка DEX — пусто).
//! Решение 491: запрет развёртывания исполнителя и классы шага — ПО СЕТИ кошелька (строка chains[]), а не одним полем
//! по площадке по умолчанию: dex_executor_refusal() и dex_executor_classes() уже считаются по своей сети площадки
//! (venue.rs → live_proof::chain_of), раньше view вызывал их один раз на площадке по умолчанию (bsc_dex, chainId 56) и
//! показывал статус BSC в блоке Arbitrum. Поле кошелька executor_deploy_refused / executor_v3_classes остаётся для
//! случая одной площадки (chains пусто, экран прежний — решение 432).
//! Настройки: нет.

use serde_json::{Value, json};

use crate::gateway::contract::VenueKind;
use crate::settings::accounts::{ROW_DEX_DEX, ROW_PROBE};

use super::state::AppState;
use super::wallets_dex::{dex_for, dex_id};

/// Строки маппинга: (строка, вид — «cex» / «dex_dex» / «probe»). Биржи — из реестра шлюза по виду. Ошибок нет.
pub async fn rows(st: &AppState) -> Vec<(String, &'static str)> {
    let mut cex = Vec::new();
    for id in st.registry.ids().await {
        if let Ok(v) = st.registry.get(&id).await
            && v.kind() == VenueKind::Cex
        {
            cex.push(id.0);
        }
    }
    cex.sort();
    let mut out: Vec<(String, &'static str)> = cex.into_iter().map(|c| (c, "cex")).collect();
    out.push((ROW_DEX_DEX.to_string(), "dex_dex"));
    out.push((ROW_PROBE.to_string(), "probe"));
    out
}

/// Всё для экрана. Адрес кошелька — только при разблокированных секретах (иначе причина). Ошибок нет.
pub async fn view(st: &AppState) -> Value {
    let a = st.settings.accounts();
    let unlocked = st.vault.status().unlocked;
    let dex = dex_id(st).await.ok().map(|d| d.0);
    // Ревью 272 (А1): обёртка из настроек DEX — экран сравнивает её с обёрткой в исполнителе v2 кошелька.
    let native_wrapped = dex.as_deref().and_then(|d| {
        let at = crate::settings::read::At {
            venue: Some(d),
            coin: None,
        };
        st.settings.text("dex.native_wrapped", at).ok()
    });
    // Решение 399: причина запрета развёртывания исполнителя (шлюз dex_executor_refusal, без сети) — кнопка
    // «Развернуть исполнитель» недоступна с этой причиной, подсказки «переразверните» не показываются. null — разрешено.
    // Решение 400: классы шага исполнителя v3 и их приёмка по записи симуляции (одни на все кошельки).
    let (deploy_refused, v3_classes) = match dex_id(st).await {
        Ok(id) => match st.registry.get(&id).await {
            Ok(v) => (v.dex_executor_refusal(), v.dex_executor_classes()),
            Err(_) => (None, Vec::new()),
        },
        Err(_) => (None, Vec::new()),
    };
    // Решение 432: площадки DEX сетей (две и больше; одна — пусто, экран прежний) — исполнитель кошелька по сети.
    let nets = super::wallets_chain::chains(st).await;
    let single_source=|key:&str|dex.as_deref().is_some_and(|v| st.settings.text(key,crate::settings::read::At {venue:Some(v),coin:None}).is_ok_and(|s|!s.is_empty()));
    let bopamm = dex.as_deref().is_some_and(|id| bopamm_configured(st, id));
    let mut wallets = Vec::new();
    for w in a.wallets() {
        let (address, why) = if !unlocked {
            (
                None,
                Some("секреты заблокированы — нужна разблокировка".to_string()),
            )
        } else {
            match dex_for(st, &w.id).await {
                Ok(v) => match v.wallet_address().await {
                    Ok(x) => (Some(x), None),
                    Err(e) => (None, Some(e.message)),
                },
                Err(e) => (None, Some(e.1)),
            }
        };
        let busy = crate::core::dex_account::open_deals_of(&st.core, &w.id);
        // Решение 401: классы шага v3 со сводкой пробы пулов через исполнитель ЭТОГО кошелька (DEX на кошелёк); нет —
        // общие (решение 400).
        let classes = match dex_for(st, &w.id).await {
            Ok(v) => v.dex_executor_classes(),
            Err(_) => v3_classes.clone(),
        };
        wallets.push(json!({
            "id": w.id,
            "fynd": single_source("fynd.network"),
            "clipper": dex.as_deref().is_some_and(|id| clipper_configured(st, id)),
            "bopamm": bopamm,
            "prop_sources": dex.as_deref().map(|v|prop_sources(st,v,nets.is_empty())).unwrap_or_default(),
            "textile": single_source("textile.base_url"),
            "kipseli": single_source("kipseli.base_url"),
            "zeroex": single_source("zeroex.base_url"),
            "name": w.name,
            "secret": w.secret,
            "executor": w.executor,
            // Решение 272: версия исполнителя по коду в сети («v1» / «v2»; null — не сверена).
            "executor_version": w.executor_version,
            // Ревью 272 (А1): обёртка из хранилища исполнителя v2 (null — не v2 / не сверен) и dex.native_wrapped.
            "executor_wrapped": w.executor_wrapped,
            "native_wrapped": native_wrapped,
            "executor_deploy_refused": deploy_refused,
            "executor_v3_classes": classes,
            "address": address,
            "address_error": why,
            "rows": a.rows_of(&w.id),
            "open_deals": busy,
            "legacy": a.legacy().as_deref() == Some(w.id.as_str()),
            "chains": by_chain(st, &w, &nets).await,
        }));
    }
    let rows: Vec<Value> = rows(st)
        .await
        .into_iter()
        .map(|(r, kind)| json!({ "row": r, "kind": kind, "wallet": a.row(&r) }))
        .collect();
    let shared: Vec<Value> = a
        .shared()
        .into_iter()
        .map(|(w, rows)| json!({ "wallet": w.id, "name": w.name, "rows": rows }))
        .collect();
    json!({
        "unlocked": unlocked,
        "dex": dex,
        "wallets": wallets,
        "rows": rows,
        "shared": shared,
    })
}

/// Решение 432: исполнитель кошелька на площадке DEX каждой сети (Wallet::at): площадка, сеть (dex.chain), адрес,
/// версия, обёртка в исполнителе и dex.native_wrapped площадки, домашняя ли (прежние поля кошелька). Одна площадка
/// DEX — пусто. Решение 491: плюс запрет развёртывания (executor_deploy_refused) и классы шага (executor_v3_classes)
/// ИМЕННО этой сети и этого кошелька — площадка сети на счёт кошелька (dex_for_at), её dex_executor_refusal() /
/// dex_executor_classes() считаются по своей сети (chainId = dex.evm_chain_id площадки); площадки/кошелька нет — null /
/// пусто. Ошибок не возвращает (сбой привязки — статус «неизвестен», как при закрытии кнопки по общему полю раньше).
async fn by_chain(
    st: &AppState,
    w: &crate::settings::accounts::Wallet,
    nets: &[(crate::core::types::VenueId, Option<String>)],
) -> Vec<Value> {
    let mut out = Vec::with_capacity(nets.len());
    for (v, chain) in nets {
        let x = w.at(&v.0);
        let at = crate::settings::read::At {
            venue: Some(&v.0),
            coin: None,
        };
        let (refused, classes) = match super::wallets_dex::dex_for_at(st, &w.id, Some(&v.0)).await {
            Ok(venue) => (venue.dex_executor_refusal(), venue.dex_executor_classes()),
            Err(_) => (None, Vec::new()),
        };
        out.push(json!({
            "venue": v.0,
            "chain": chain,
            "fynd": st.settings.text("fynd.network",at).is_ok_and(|s|!s.is_empty()),
            "clipper": clipper_configured(st, &v.0),
            "bopamm": bopamm_configured(st, &v.0),
            "prop_sources": prop_sources(st,&v.0,nets.len()<=1),
            "textile": st.settings.text("textile.base_url",at).is_ok_and(|s|!s.is_empty()),
            "kipseli": st.settings.text("kipseli.base_url",at).is_ok_and(|s|!s.is_empty()),
            "zeroex": st.settings.text("zeroex.base_url",at).is_ok_and(|s|!s.is_empty()),
            "home": w.home() == v.0,
            "executor": x.executor,
            "executor_version": x.executor_version,
            "executor_wrapped": x.executor_wrapped,
            "native_wrapped": st.settings.text("dex.native_wrapped", at).ok(),
            "executor_deploy_refused": refused,
            "executor_v3_classes": classes,
        }));
    }
    out
}

fn clipper_configured(st: &AppState, venue: &str) -> bool {
    let at = crate::settings::read::At { venue: Some(venue), coin: None };
    st.settings.integer("dex.evm_chain_id",at).is_ok_and(|chain| chain > 0)
        && st.settings.text("clipper.base_url",at).is_ok_and(|url| !url.is_empty())
        && st.settings.get_exact("clipper.pools", &crate::settings::scope::Scope::Venue(venue.into()))
            .is_some_and(|v| v.as_array().is_some_and(|rows| !rows.is_empty()))
}

fn bopamm_configured(st: &AppState, venue: &str) -> bool {
    let at = crate::settings::read::At { venue: Some(venue), coin: None };
    st.settings.integer("dex.evm_chain_id", at).ok() == Some(1)
        && st.settings.get_exact("bopamm.roles", &crate::settings::scope::Scope::Venue(venue.into()))
            .is_some_and(|v| v.as_array().is_some_and(|rows| !rows.is_empty()))
}

fn prop_sources(st:&AppState, venue:&str, alone:bool) -> Vec<&'static str> {
    use crate::settings::{dex_key::venue_suffix, read::At, scope::Scope};
    if st.settings.integer("dex.evm_chain_id",At{venue:Some(venue),coin:None}).ok()!=Some(1){return Vec::new();}
    let Ok(suffix)=venue_suffix(&st.settings,venue,alone) else{return Vec::new()};
    ["kaliber-lambda-ethereum","taurus-lambda-ethereum"].into_iter().filter(|id|{
        let key=suffix.as_ref().map_or_else(||(*id).to_string(),|chain|format!("{id}{chain}"));
        st.settings.get_exact("dex.propamm_price_slots",&Scope::Dex(key)).is_some_and(|x|x.as_array().is_some_and(|a|!a.is_empty()))
    }).collect()
}
