//! Исходящие суточные книги Telegram. Работает в своей задаче, использует настройки
//! telegram.daily_time_utc, telegram.daily_chat_ids и секрет telegram.daily_bot_token.
#[cfg(test)]
mod files_test;
mod reports;
mod spool;
#[cfg(test)]
mod tests;

use super::{
    ctl::report::REPORTS,
    outbound::{self, Channel},
    telegram::TgError,
};
use crate::{
    core::Core,
    report::{self, archive::Archive},
    secrets::Vault,
    settings::{outbound::utc_time, read::At},
    storage::journal::Level,
};
use chrono::{DateTime, NaiveTime, TimeDelta, Utc};
use spool::Spool;
use std::time::Duration;

fn configured_time(core: &Core) -> Option<NaiveTime> {
    core.inner
        .deps
        .settings
        .text("telegram.daily_time_utc", At::default())
        .ok()
        .and_then(|s| utc_time(&s))
}

async fn log(core: &Core, level: Level, text: String) {
    core.inner
        .deps
        .log(level, "notify.daily", None, text, None)
        .await;
}

/// Проверяем текущие разрешённые адресаты перед каждым файлом, включая удаление чата во время сборки.
async fn deliver(
    core: &Core,
    http: &reqwest::Client,
    vault: &Vault,
    spool: &Spool,
    slot: DateTime<Utc>,
) -> Result<(), String> {
    let settings = &core.inner.deps.settings;
    for chat in spool.audience().await? {
        if spool
            .chat_waits(chat.clone(), Utc::now().timestamp())
            .await?
        {
            continue;
        }
        let pending = spool.pending(chat.clone(), Utc::now().timestamp()).await?;
        for (report, part) in pending {
            let Some(time) = configured_time(core) else {
                return Ok(());
            };
            let now = Utc::now();
            let latest = now.date_naive().and_time(time).and_utc();
            if latest <= now && latest.date_naive() > slot.date_naive() {
                return Ok(());
            }
            let Ok((tg, current)) = outbound::build(http, settings, vault, Channel::Daily) else {
                return Ok(());
            };
            if !current.contains(&chat) {
                break;
            }
            let Some(f) = spool
                .claim(chat.clone(), report.clone(), part, now.timestamp())
                .await?
            else {
                continue;
            };
            let res = tg
                .send_document(&chat, &f.name, report::XLSX_MIME, f.bytes, &f.caption)
                .await;
            let (status, retry, level, outcome) = match res {
                Ok(id) => ("sent", 0, Level::Info, format!("доставлен, сообщение {id}")),
                Err(e) => {
                    let (status, due) = match &e {
                        TgError::Api {
                            retry_after: Some(s),
                            ..
                        } if *s > 0 => ("rate_limited", Utc::now().timestamp().saturating_add(*s)),
                        TgError::Api {
                            error_code: Some(429),
                            ..
                        } => ("rate_limited", 0),

                        _ => ("retry", 0),
                    };
                    (status, due, Level::Warn, format!("будет повторён: {e}"))
                }
            };
            spool
                .finish(chat.clone(), report, part, status, retry)
                .await?;
            log(
                core,
                level,
                format!(
                    "Автоотчёты за сутки до {slot}: файл {} в чат {chat} {outcome}",
                    f.name
                ),
            )
            .await;
            if status != "sent" {
                break;
            }
        }
    }
    Ok(())
}

async fn tick(
    core: &Core,
    http: &reqwest::Client,
    vault: &Vault,
    spool: &Spool,
    time: NaiveTime,
) -> Result<(), String> {
    let now = Utc::now();
    let Ok((_, chats)) = outbound::build(http, &core.inner.deps.settings, vault, Channel::Daily)
    else {
        return Ok(());
    };
    let Some(slot) = spool.slot_for(now, time, chats).await? else {
        return Ok(());
    };
    let mut source = None;
    let mut snapshot = None;
    let w = reports::window(slot, now);
    for (id, title, _) in REPORTS {
        if configured_time(core).is_none()
            || outbound::build(http, &core.inner.deps.settings, vault, Channel::Daily).is_err()
        {
            return Ok(());
        }
        if !spool.begin(id, Utc::now().timestamp()).await? {
            continue;
        }
        if source.is_none() {
            source = Some(Archive::open(core).await);
        }
        let built = match source.as_ref() {
            Some(Ok(archive)) => reports::build(core, archive, spool, id, w, &mut snapshot).await,
            Some(Err(e)) => Err(e.clone()),
            None => unreachable!(),
        };
        if let Err(e) = built {
            log(
                core,
                Level::Warn,
                format!("Автоотчёты за сутки до {slot}: «{title}» — сборка прервана: {e}"),
            )
            .await;
        }
        // Готовая книга отправляется, пока остальные ещё не собраны.
        deliver(core, http, vault, spool, slot).await?;
    }
    deliver(core, http, vault, spool, slot).await
}

pub async fn run(core: Core, http: reqwest::Client, vault: Vault) {
    let mut spool = None;
    let mut last_error = None;
    let mut changes = core.inner.deps.settings.subscribe();
    let mut secrets = crate::secrets::vault::changes();
    loop {
        let time = configured_time(&core);
        let result: Result<(), String> = async {
            if let Some(t) = time
                && outbound::build(&http, &core.inner.deps.settings, &vault, Channel::Daily).is_ok()
            {
                if spool.is_none() {
                    spool = Some(
                        Spool::open(core.inner.deps.data_dir.join("telegram_daily.sqlite")).await?,
                    );
                }
                if let Some(s) = &spool {
                    tick(&core, &http, &vault, s, t).await?;
                }
            }
            Ok(())
        }
        .await;
        match result {
            Err(e) if last_error.as_ref() != Some(&e) => {
                log(&core, Level::Error, format!("Автоотчёты: {e}")).await;
                last_error = Some(e);
            }
            Ok(()) => last_error = None,
            _ => {}
        }
        // Минутная точность настроек; перед ближайшим сроком просыпаемся ровно к HH:MM.
        let now = Utc::now();
        let wait = time
            .and_then(|t| {
                let at = now.date_naive().and_time(t).and_utc();
                ((if at > now {
                    at
                } else {
                    at + TimeDelta::days(1)
                }) - now)
                    .to_std()
                    .ok()
            })
            .unwrap_or(Duration::from_secs(60))
            .min(Duration::from_secs(60));
        tokio::select! {
            _ = tokio::time::sleep(wait), if time.is_some() => {}
            r = secrets.changed() => if r.is_err() {return},
            open = async {
                loop {
                    match changes.recv().await {
                        Ok(c) if c.name.starts_with("telegram.daily_")
                            || c.name == "notify.telegram_api_url" || c.name == "http.timeout_ms" => return true,
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return true,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                    }
                }
            } => if !open { return; }
        }
    }
}
