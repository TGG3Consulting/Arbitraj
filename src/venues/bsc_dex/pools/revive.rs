//! Файл: venues/bsc_dex/pools/revive.rs
//! Что делает: возвращает в чтение пулы, брошенные из-за незаданной настройки (GatewayError.setting), когда эту
//! настройку задали: по изменению из рассылки настроек (on_settings) или, если рассылка отстала и изменения пропущены,
//! по всем настройкам, у которых теперь есть значение (on_lagged). Вернувшиеся сразу читаются на последнем блоке.
//! Кто вызывает: pools/session.rs; тип Abandoned — pools/reload.rs, pools/admit.rs.
//! От чего зависит: reload.rs (retry, Missing), loading.rs, settings (Store::all), node/heads.rs.
//! Настройки: своих нет.

use super::discover::Found;
use super::listen::{End, Pools};
use super::reload::{Missing, retry};

/// Брошенный пул: индекс в пачке и незаданная настройка, из-за которой он не прочитался (None — другая причина:
/// такой пул ждёт переподключения пачки).
#[derive(Debug, Clone)]
pub struct Abandoned {
    pub i: usize,
    pub setting: Option<String>,
}

/// Брошенные из-за настройки `name` пулы снова ждут чтения (её задали): попытки — заново, в «загружаются» — снова.
/// Остальные брошенные и ждущие не трогаются. Принимает: пулы пачки, брошенные, имя заданной настройки.
/// Возвращает: вернувшиеся. Ошибок нет.
pub fn revive(
    p: &Pools,
    pools: &[(Found, String)],
    abandoned: &mut Vec<Abandoned>,
    name: &str,
) -> Vec<Missing> {
    let (back, keep): (Vec<Abandoned>, Vec<Abandoned>) = std::mem::take(abandoned)
        .into_iter()
        .partition(|a| a.setting.as_deref() == Some(name));
    *abandoned = keep;
    p.loading.add(back.iter().map(|a| &pools[a.i].0));
    back.into_iter()
        .map(|a| Missing { i: a.i, tries: 0 })
        .collect()
}

/// Сообщение рассылки настроек: изменение — on_settings; отставание (Lagged, изменения пропущены) — on_lagged.
/// Принимает: окружение, пулы пачки, ждущие, брошенные, принятое. Возвращает: false — рассылка закрыта (настроек
/// больше не будет). Ошибки: End — ядро закрыло канал.
pub async fn on_recv(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<Abandoned>,
    got: Result<crate::settings::live::Change, tokio::sync::broadcast::error::RecvError>,
) -> Result<bool, End> {
    use tokio::sync::broadcast::error::RecvError;
    match got {
        Ok(change) => on_settings(p, pools, missing, abandoned, &change).await?,
        Err(RecvError::Lagged(_)) => on_lagged(p, pools, missing, abandoned).await?,
        Err(RecvError::Closed) => return Ok(false),
    }
    Ok(true)
}

/// Изменилась настройка: если её задали (новое значение есть), пулы, брошенные из-за неё, — снова в чтение.
/// Принимает: окружение, пулы пачки, ждущие, брошенные, изменение. Ошибки: End — ядро закрыло канал.
pub async fn on_settings(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<Abandoned>,
    change: &crate::settings::live::Change,
) -> Result<(), End> {
    if change.new.is_none() {
        return Ok(());
    }
    bring_back(
        p,
        pools,
        missing,
        abandoned,
        std::slice::from_ref(&change.name),
    )
    .await
}

/// Рассылка изменений настроек отстала (Lagged — изменения пропущены): возвращаются пулы, брошенные из-за
/// настройки, у которой теперь есть значение (в любой области). Ошибки: End — ядро закрыло канал.
pub async fn on_lagged(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<Abandoned>,
) -> Result<(), End> {
    let set: std::collections::HashSet<String> = p
        .ctx
        .settings
        .all()
        .into_iter()
        .map(|(n, _, _)| n)
        .collect();
    let names: Vec<String> = abandoned
        .iter()
        .filter_map(|a| a.setting.clone())
        .filter(|n| set.contains(n))
        .collect();
    bring_back(p, pools, missing, abandoned, &names).await
}

/// Возвращает в чтение пулы, брошенные из-за настроек `names`, и сразу пробует их на последнем блоке (попытки обычных
/// ждущих не тратятся); не прочитались — к ждущим. Ошибки: End — ядро закрыло канал.
async fn bring_back(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<Abandoned>,
    names: &[String],
) -> Result<(), End> {
    let mut back = Vec::new();
    for n in names {
        back.extend(revive(p, pools, abandoned, n));
    }
    if back.is_empty() {
        return Ok(());
    }
    p.loading.publish(p).await;
    if let Some(h) = p.heads.latest() {
        retry(p, pools, &mut back, abandoned, h.number).await?;
    }
    missing.extend(back);
    Ok(())
}
