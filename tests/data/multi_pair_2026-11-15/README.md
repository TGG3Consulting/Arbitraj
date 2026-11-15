# multi_pair_2026-11-15 — эталонные снимки решения 295 (шаг 0)

Текстовые снимки поведения парных пулов и многомонетных видов без dex.multi_pair, записанные КОДОМ ДО правки
(ветка t126/multi-pair от cab52fda) на записанных данных других папок (сами данные сети здесь не хранятся):

- `routes_2026-10-29.txt`, `routes_2026-10-30.txt` — tests/bsc_dex_multi_pair_golden_routes.rs (записи routes_*);
- `directory_2026-10-24.txt` — tests/bsc_dex_multi_pair_golden_dir.rs (indexers_2026-10-24);
- `stable_2026-11-15.txt` — tests/bsc_dex_multi_pair_golden_stable.rs (nerve_/ellipsis_/balancer_acsi_2026-11-15, curve_ng_2026-11-14);
- `batch_load_2026-10-25.txt` — tests/bsc_dex_batch_load.rs (arb_node_responses_3_2026-10-25).

Перезапись — только `ARB_GOLDEN_WRITE=1` (tests/golden_common/mod.rs) и только на неизменённом коде.
