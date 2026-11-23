# Пулы кода AlgebraV1 — диагностическая запись публичного RPC Arbitrum One (zyber_public_2026-11-23)

Записано tools/bsc_record/thena_fusion_run.py (только чтение). Состав файлов — шапка thena_fusion.py.

- chainId: 42161
- фабрика 0x9c2abd632771b433e5e7507bcaa41ca3b25d8544, poolDeployer 0x24e85f5f94c6017d2d87b434394e87df4e4d56e3, keccak кода деплоера 0x4e463f8f46585d631236803903b92ccc60a7e196b7b21baaba258c035c4e0b27

- пул 0x308c5b91f63307439fdb51a9fa4dfc979e2ed6b0: блок 526909448, tickSpacing 60, keccak кода 0xc7d8a1b7d4283215cff638490146a192ae9bcad4ced8c67e531feea37168ffce, CREATE2 по хешу ['0x6ec6c9c8091d160c0aa74b2b14ba9c1717e95093bd3ac085cee99a49aab294a4']; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 72, liquidityTotal > 0 — 72
- пул 0x4ef385d20247f5d0f9cd2e3f716b9a55e71df934: блок 526909436, tickSpacing 60, keccak кода 0x23520308f7b271d944ffeb6a3e619221bea3a5a928a2d865df047eaf0e570960, CREATE2 по хешу ['0x6ec6c9c8091d160c0aa74b2b14ba9c1717e95093bd3ac085cee99a49aab294a4']; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 0, liquidityTotal > 0 — 0

## Источник и границы замера

Обе записи получены через https://arb1.arbitrum.io/rpc. Официальный список RPC опубликован на https://docs.arbitrum.io/arbitrum-essentials/reference/node-providers. Запись использует публичную ноду для диагностики. Приёмка на узле бота по T487-01 и T487-02 проводится отдельно.

На момент чтения у WETH/USDC последнее обновление оракула произошло 78 063 секунды назад, у WETH/USDC.e — 230 секунд назад. На одном состоянии каждого пула комиссия getFee (миллионные доли) на момент блока и сдвиги +1 и +3 секунды составила соответственно 500/500/500 и 499/499/499. У каждого пула записаны оба направления, четыре размера (1, 10, 100, 1000 б.п. баланса входа) и четыре времени. Виртуальный обмен использует eth_call с временным bytecode и выбранным временем блока.

200 соседних заголовков включают оба блока записей. 146 интервалов имели длину 0 секунд и 53 — 1 секунду. Исходные пулы и оракулы были зафиксированы до ограничения частоты RPC. Для завершения записи заголовки получены меньшими пачками и с паузой. Горизонт и размер части для рабочего узла из этих наблюдений не определяются.

## SHA-256 файлов

9fb7d70fc2295b116163950effe096d0147cbbf25c282ad6f12f3e0e096088ff  factory.json
04c3062eeff0bf908338a82abb4c16b83201ddfbd47027589c03b82a6c5e1f1c  geckoterminal_pools.json
5697db521bcc79f6a1925133c0919f81356ce02c69e131dc80e58ccd694eb11f  heads.json
a62716ab61d612568a1040564d9c577ed85d4dc3c1885115c04f19de68d9570c  meta.json
700957bba7ebcde3a302ffb344091a2446231646bb5b523a887dfa3af86fc213  pool_0x308c5b91f63307439fdb51a9fa4dfc979e2ed6b0.json
3ac4b777be23f343d8a98c414bb13037cf3312f1deb7e40f6d749185572892e1  pool_0x4ef385d20247f5d0f9cd2e3f716b9a55e71df934.json
8fb3f74ceef3bf7bd266827503f8b1cc8a3327c188641da724687eca83935f1c  sourcify.json
