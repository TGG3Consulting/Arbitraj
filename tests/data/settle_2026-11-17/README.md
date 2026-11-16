# Цепочки логов одного пула из квитанций блоков — 17.11.2026 (решение 357)

Записал серверный ИИ со своего узла BSC (geth 1.8.2, http://127.0.0.1:8545, только чтение) по промпту
docs/notes/t154-settle-2026-11-16.md, раздел 5. Исходные файлы записи (не в репозитории, у координатора; SHA-256 — как в
README записи): receipts_LB_133991544.json 16000dd9ea185da589fc907cac58e71001e5b1520a816889229250e47452ce6,
receipts_Bin_133943958.json b03aee966aed6b1b55b90a57ddf5e51d8360f8412e3930e42b348128d4828bc3,
receipts_Algebra_134001089.json 5c2f639e25fa1466f2e6c31dbad1aed4c54c74579e674172219612bcf6067e44,
part8.json e8468fe4fc4930c994796496d25e44641815c1a9cfcd71bc8b8603008bb16339,
hist_part.json 7d8a5051bc96bd43036fc10990a3d02a5e156005e94d6d9f00b76790d03be345.

Здесь — только нужное тестам, объекты логов и ответов дословно (Python: все логи из ВСЕХ квитанций блока, у которых
address = пул (LB, Algebra) или address = BinPoolManager и topics[1] = PoolId (Infinity Bin); поле receipts — сколько
квитанций в блоке):

- `algebra_134001089.json` — thena-v3 (Algebra Integral) 0x8829abfa1a7b017078195c10a966d7411a0c9515, блок 134001089,
  tx 0x8333…236c: Fee(uint16) 1204 (logIndex 0x115, topic0 0x598b9f04…) и Swap(address,address,int256,int256,uint160,
  uint128,int24,uint24,uint24) (0x119, topic0 0x121cb44e…; overrideFee 1204, pluginFee 0); между ними — логи токенов.
  `state` — part8.json Algebra[0].state дословно: globalState, fee, liquidity, safelyGetStateOfAMM на 134001088 и
  134001089. События SwapFee (topic0 0x9443903d…) в записи нет ни у одного адреса.
- `lb_133991544.json` — LB V2.1 BTC.b/WBNB 0x717d06b4d8bc6b71d5a2c45d8cc417930aec073d (getFactory = LBFactory
  0x8e42…fd4e), блок 133991544, tx 0xd420…d6d9: два Swap (0x54 бин 8399652, 0x55 бин 8399740). Состояние на N−1 и N
  узел не отдал (historical state is not available — hist_part.json).
- `bin_133943958.json` — Infinity Bin, BinPoolManager 0xC697…A4a9, PoolId 0xa3aa…040f, блок 133943958: два Swap из
  tx 0x1d и 0x1e, activeId обоих 8387666. getSlot0 на N−1 и N узел не отдал.

topic0 проверены keccak256 сигнатур: Swap LB (address,address,uint24,bytes32,bytes32,uint24,bytes32,bytes32) 0xad7d6f97,
Swap Infinity Bin (bytes32,address,int128,int128,uint24,uint24,uint16) 0x3e8aae37, Fee(uint16) 0x598b9f04,
SwapFee(address,uint24,uint24) 0x9443903d.

SHA-256 файлов этой папки:
- ec68d9a14ac2ad0250146ce954ce03c57bc86f1e057729e1534bebb109e0bdf9  algebra_134001089.json
- 1e235101775d9a50c510c1c02e54410783fdcf7c996cc16f9c4cf59c63eccc7a  bin_133943958.json
- ace6ac093d51d36df6ff495000e2f90a71abad41c757b9b96721db57d5ea07d8  lb_133991544.json

Используется: tests/bsc_dex_settle_records_357.rs.
