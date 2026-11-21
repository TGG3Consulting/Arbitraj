# Arbitrum узел — проба (только чтение), 2026-11-19

Узел: arb-node (37.59.21.151), Nitro v3.12.4, Arbitrum One (chainId 42161), синхронизирован.
Все вызовы — чтение/симуляция, состояние узла не менялось, перезапусков не было.

## Файлы
- 00_node_info.txt — версия, флаги, синхронизация, блок, порты, фаервол, доступ бота (п.1)
- 01_eth_chainId.json — eth_chainId
- 02_eth_call_state_override.json — eth_call balanceOf(USDT) со state override.
    USDT=0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9. Найден слот mapping balances index=51,
    через stateDiff подменён баланс холдера -> balanceOf вернул 0xdeadbeef. Плюс демо code-override.
- 03_eth_simulateV1.json — eth_simulateV1, простой перевод 1 ETH (с stateOverride баланса отправителя)
- 04_eth_estimateGas.json — eth_estimateGas простого перевода (0x5765 = 22373)
- 05_nodeinterface_gasEstimateComponents.json — NodeInterface 0x..C8 gasEstimateComponents(to,false,0x);
    результат: gasEstimate, gasEstimateForL1, baseFee, l1BaseFeeEstimate
- 06_ws_eth_subscribe_newHeads.json — WS eth_subscribe newHeads (подтверждение + 3 уведомления)
- 07_ws_eth_subscribe_logs.json — WS eth_subscribe logs (подтверждение + 3 уведомления)
- geckoterminal/dexes_page1.json — GET api/v2/networks/arbitrum/dexes (всего 1 страница, 84 декса; last=page1)

## Инструменты (в архиве, для воспроизводимости)
- eth.py — pure-python keccak256 + JSON-RPC; probe.py — пункты 2; ws.mjs — WS-подписки (node)

## SHA-256 файлов
54dbe730deb213fe37369920cefb2a3415ecc730e87affc04a74b095a66ab100  00_node_info.txt
a3b6babcdec97fa1098737f186b1ad4781750745185cbcba135c2d6d6cd154f7  01_eth_chainId.json
d3321af1cb6ce70bf4badb2a4e239d6e2c8ab6194b9669fc45d5e7ff78a980f5  02_eth_call_state_override.json
93be9d27fe6d1107aba459919a5fbce875625d8ba100f780a64e76890c1c9990  03_eth_simulateV1.json
6c0c8976758609be5cbe8af77a4b9f30632e0b167b83d2e4ede6b9062e390ec8  04_eth_estimateGas.json
9bc76b58adb30f59468b250bf589a4468e701e28d1afda8198e0b1d45a84783c  05_nodeinterface_gasEstimateComponents.json
1189cca977f0345061d90e0a0ed4704430ec59a9548f809e40a4d4c6c816fa06  06_ws_eth_subscribe_newHeads.json
5a7cadcaaa886108c22e76748706d52670cdebcad0366e3b7e9015fd69cbd805  07_ws_eth_subscribe_logs.json
027a7edc5ebb617ca86f19307d4b42b216fb38fbe2f9c23cef63e80e43c2cdab  geckoterminal/dexes_page1.json
