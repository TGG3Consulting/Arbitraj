//! Файл: main.rs
//! Что делает: запуск системы — разбор аргументов, база, настройки, журнал, секреты,
//! уведомления (и приём кнопок Telegram), шлюз, ядро и веб-сервер интерфейса (с раздачей собранного интерфейса, если задан --ui-dir).
//! Кто вызывает: операционная система (запуск программы).
//! От чего зависит: все модули. Адаптеры площадок (BingX, DEX в сети BSC) регистрируются здесь.
//! Настройки: свои — нет; аргументы запуска — cli.rs.

use std::process::ExitCode;

use tokio::sync::broadcast;

use arb::api::state::AppState;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::secrets::Vault;
use arb::settings::live::Watch;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};

/// Имя файла базы в папке данных.
const DB_FILE: &str = "arb.sqlite";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Собирает и запускает все компоненты.
/// Ошибки: неверные аргументы, нет доступа к папке данных, база, занят адрес.
async fn run() -> Result<(), String> {
    let args = arb::cli::parse(std::env::args().skip(1))?;
    std::fs::create_dir_all(&args.data_dir)
        .map_err(|e| format!("Папка данных {:?}: {e}", args.data_dir))?;
    let db = Db::open(&args.data_dir.join(DB_FILE)).map_err(|e| e.to_string())?;

    let (changes, _) = broadcast::channel(args.live_buffer);
    let (journal_live, _) = broadcast::channel(args.live_buffer);
    let journal = Journal::new(db.clone(), journal_live.clone());
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .map_err(|e| e.to_string())?;
    let vault = Vault::locked(arb::secrets::file::path_in(&args.data_dir));
    let http = reqwest::Client::builder()
        // Редиректы выключены: в адресе запроса может быть токен, его нельзя отдавать другому адресу.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let notifier = Notifier {
        http: http.clone(),
        settings: settings.clone(),
        vault: vault.clone(),
        journal: journal.clone(),
    };

    // Изменения настроек — в журнал и в Telegram (если событие включено).
    tokio::spawn(arb::notify::settings_watch::run(
        Watch::new(changes.subscribe(), vec![]),
        notifier.clone(),
    ));

    // Адаптеры площадок: создаются здесь и регистрируются в шлюзе; ядро видит их только через шлюз.
    let registry = Registry::new();
    let raw = arb::storage::raw_log::RawLog::start(args.data_dir.clone(), settings.clone());
    let bingx_ctx = arb::venues::bingx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::bingx::Bingx::new(
            bingx_ctx,
        )))
        .await;
    let dex_ctx = arb::venues::bsc_dex::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw,
        pace: Default::default(),
    };
    let dex = arb::venues::bsc_dex::BscDex::new(dex_ctx);
    // Изменение промежуточных монет (решение 52) — пересборка подписки DEX на лету.
    dex.watch_settings(changes.subscribe());
    registry.register(std::sync::Arc::new(dex)).await;

    // Ядро: работает с площадками только через шлюз; интерфейс читает его состояние через AppState.core.
    let core = arb::core::Core::start(arb::core::deps::Deps {
        gw: registry.clone(),
        settings: settings.clone(),
        journal: journal.clone(),
        notifier: notifier.clone(),
        db: db.clone(),
        changes: changes.clone(),
        buffer: args.live_buffer,
    })
    .await;

    // Кнопки Telegram → команды ядру (CommandTx, core/types/control.rs). wake — сигнал «секреты
    // изменились» от api/secrets.rs: приём начинает заново с актуальным токеном.
    let secrets_wake = std::sync::Arc::new(tokio::sync::Notify::new());
    tokio::spawn(arb::notify::updates::run(
        arb::notify::updates::Updates {
            http: http.clone(),
            settings: settings.clone(),
            vault: vault.clone(),
            journal: journal.clone(),
            commands: core.command_channel(),
            wake: secrets_wake.clone(),
        },
        Watch::new(
            changes.subscribe(),
            arb::notify::updates::WATCH_PREFIXES
                .iter()
                .map(|p| p.to_string())
                .collect(),
        ),
    ));

    let state = AppState {
        db,
        settings,
        journal: journal.clone(),
        registry,
        vault,
        notifier: notifier.clone(),
        http,
        changes,
        journal_live,
        core,
        secrets_wake,
    };
    let guard = arb::api::guard::Guard::new(&args.listen.to_string(), &args.allow_origins);
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .map_err(|e| format!("Не удалось занять адрес {}: {e}", args.listen))?;

    let started = match &args.ui_dir {
        Some(_) => format!("Система запущена, интерфейс: http://{}", args.listen),
        None => format!(
            "Система запущена без раздачи интерфейса (--ui-dir не задан), API: http://{}",
            args.listen
        ),
    };
    tracing::info!("{started}");
    let _ = journal
        .write(Level::Info, "system.started", None, started.clone(), None)
        .await;
    notifier.send("system.started", None, &started).await;

    arb::api::server::serve(listener, state, guard, args.ui_dir)
        .await
        .map_err(|e| format!("Веб-сервер остановился: {e}"))
}
