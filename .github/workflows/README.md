# workflows — CI

`ci.yml` — на каждый PR и пуш в `main`: `cargo fmt --check`, `cargo clippy -D warnings`, сборка, тесты.

`linux.yml` — сборка для Linux (сервер), запуск вручную.

`windows.yml` — сборка для Windows одним архивом «arb-windows».

`tests.yml` — полный прогон тестов параллельными частями (8 частей + clippy) на машинах GitHub: на пуш в ветки
`test/**` и вручную. Ветки `test/**` служат только для прогона, никуда не сливаются.
