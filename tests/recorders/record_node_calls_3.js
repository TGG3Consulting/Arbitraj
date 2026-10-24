// Файл: tests/recorders/record_node_calls_3.js
// Что делает: запись ЖИВЫХ ответов узла BSC для пачечного чтения состояния пулов (решение 61, PR #47,
//   pools/load_batch.rs) — то же, что record_node_calls_3 в tests/bsc_dex_node_calls_3.rs, но на JavaScript: запускается
//   в консоли браузера (обычный fetch, без библиотек). Всё за один проход на одном блоке (публичный узел не отдаёт
//   старые блоки):
//   0) eth_blockNumber — блок закрепляется для всех вызовов;
//   1) ступень 1 — список из tests/data/node_calls_3_2026-10-24.json (V2 getReserves; V3 slot0 + liquidity;
//      Infinity CL — CLPoolManager getSlot0 + getLiquidity; Uniswap V4 — StateView getSlot0 + getLiquidity);
//   2) по тику из slot0 / getSlot0 — слова [w − TICK_WORDS, w + TICK_WORDS] (как load_v2v3::word_range): V3 — TickLens
//      getPopulatedTicksInWord, Infinity CL — getPoolBitmapInfo, V4 — StateView getTickBitmap;
//   3) по установленным битам слов CL — данные тиков: getPoolTickInfo / getTickLiquidity (tick = (слово·256 + бит)·tickSpacing);
//   в конце — те же пачки Multicall3.aggregate3 ступеней 1–3, что шлёт #47 (вызовы ступени всех пулов подряд, части по
//   BATCH, allowFailure = true).
//   Каждый вызов ступеней — отдельным eth_call. Итог — {about, block, results: [{name, request, response}]} в
//   window.__rec (имена вызовов — как у Rust-рекордера: «s1 …», «s2 …», «s3 …»).
// Кто вызывает: человек — вставить в консоль браузера.
// От чего зависит: ничего (fetch). Пулы и ступень 1 — из node_calls_3_2026-10-24.json (write_node_calls_3).
// Настройки: TICK_WORDS = dex.tick_words, BATCH = dex.verify_batch (значения у Тиграна — 2 и 200).
//
// ABI вручную: селекторы — из sol!-описаний бота (pools/abi_v2v3.rs, abi_v4.rs, node/multicall.rs); int16 / int24 —
// 32 байта в дополнительном коде (знак!). Адреса — официальные страницы адресов (research/flow/bsc_dex.md, bsc_v4.md).

(async () => {
  const NODE = 'https://bsc-dataseed.bnbchain.org';
  const TICK_WORDS = 2;
  const BATCH = 200;
  const PARALLEL = 8; // одновременных запросов (только скорость записи, на данные не влияет)
  const MULTICALL3 = '0xca11bde05977b3631167028862be2a173976ca11';
  const CL_MANAGER = '0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b';
  const STATE_VIEW = '0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4';
  const PCS_TICK_LENS = '0x9a489505a00ce272eaa5e07dba6491314cae3796';
  const UNI_TICK_LENS = '0xd9270014d396281579760619ccf4c3af0501a47c';
  const SEL = {
    lens: '351fb478', // getPopulatedTicksInWord(address,int16)
    bitmapInf: '7c352ef6', // getPoolBitmapInfo(bytes32,int16)
    bitmapV4: '1c7ccb4c', // getTickBitmap(bytes32,int16)
    tickInf: '5aa208a4', // getPoolTickInfo(bytes32,int24)
    tickV4: 'caedab54', // getTickLiquidity(bytes32,int24)
    aggregate3: '82ad56cb', // aggregate3((address,bool,bytes)[])
  };
  // [имя, вид, адрес, PoolId, tickSpacing]
  const POOLS = [
  ["CAKE pancakeswap_v2 0x0ed7e52944161450477ee417de9cd3a859b14fd0", "v2", "0x0ed7e52944161450477ee417de9cd3a859b14fd0", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["CAKE pancakeswap-v3-bsc 0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1", "pcs_v3", "0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1", "0x0000000000000000000000000000000000000000000000000000000000000000", 50],
  ["CAKE pancakeswap-v3-bsc 0x133b3d95bad5405d14d53473671200e9342896bf", "pcs_v3", "0x133b3d95bad5405d14d53473671200e9342896bf", "0x0000000000000000000000000000000000000000000000000000000000000000", 50],
  ["CAKE pancakeswap-v3-bsc 0xafb2da14056725e3ba3a30dd846b6bbbd7886c56", "pcs_v3", "0xafb2da14056725e3ba3a30dd846b6bbbd7886c56", "0x0000000000000000000000000000000000000000000000000000000000000000", 10],
  ["CAKE uniswap-bsc 0xfe4fe5b4575c036ac6d5cccfe13660020270e27a", "uni_v3", "0xfe4fe5b4575c036ac6d5cccfe13660020270e27a", "0x0000000000000000000000000000000000000000000000000000000000000000", 60],
  ["CAKE pancakeswap-infinity-clmm 0x54c27041dfa246727d9351613eb35da028ddf377225d8db9e68ca3b569b5ba24", "infinity_cl", "0x0000000000000000000000000000000000000000", "0x54c27041dfa246727d9351613eb35da028ddf377225d8db9e68ca3b569b5ba24", 50],
  ["CAKE pancakeswap-infinity-clmm 0x47516855520496b84a169f7bb92ace7ffb6e8c535bccb52a308ccff113aeccfb", "infinity_cl", "0x0000000000000000000000000000000000000000", "0x47516855520496b84a169f7bb92ace7ffb6e8c535bccb52a308ccff113aeccfb", 50],
  ["CAKE pancakeswap_v2 0xde52cff316d8a70256bee647073312c1aa7ee2cf", "v2", "0xde52cff316d8a70256bee647073312c1aa7ee2cf", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["CAKE uniswap-v4-bsc 0x4630fb37706efaf996be1ee54b6841f8d9548ac42ae6cebc02511d891726d1e6", "uni_v4", "0x0000000000000000000000000000000000000000", "0x4630fb37706efaf996be1ee54b6841f8d9548ac42ae6cebc02511d891726d1e6", 25],
  ["CAKE pancakeswap_v2 0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b", "v2", "0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["CAKE pancakeswap-v3-bsc 0x8ec186cd1ad51c380bd23fde29f852226647616c", "pcs_v3", "0x8ec186cd1ad51c380bd23fde29f852226647616c", "0x0000000000000000000000000000000000000000000000000000000000000000", 10],
  ["CAKE pancakeswap_v2 0x804678fa97d91b974ec2af3c843270886528a9e6", "v2", "0x804678fa97d91b974ec2af3c843270886528a9e6", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["CAKE uniswap-bsc 0x8e8410f4878079fc03a287a7ded4a65afb87cc06", "uni_v3", "0x8e8410f4878079fc03a287a7ded4a65afb87cc06", "0x0000000000000000000000000000000000000000000000000000000000000000", 200],
  ["CAKE pancakeswap-infinity-clmm 0x737a7d974a19bafb34c8d74d898188c9b59689b91f291fa6ade69f71fa0f5afa", "infinity_cl", "0x0000000000000000000000000000000000000000", "0x737a7d974a19bafb34c8d74d898188c9b59689b91f291fa6ade69f71fa0f5afa", 10],
  ["CAKE pancakeswap_v2 0x5ffc1857a39a8d63e556e988f8e28ce6cf7d0db9", "v2", "0x5ffc1857a39a8d63e556e988f8e28ce6cf7d0db9", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["CAKE pancakeswap-v3-bsc 0xeb7528398b2725e1e0374734a87320132223a5c6", "pcs_v3", "0xeb7528398b2725e1e0374734a87320132223a5c6", "0x0000000000000000000000000000000000000000000000000000000000000000", 10],
  ["CAKE pancakeswap-infinity-clmm 0xaf12088595c0d5615f1e9da6033a435705d60037c2a4230dd5e135aab9dcfeb5", "infinity_cl", "0x0000000000000000000000000000000000000000", "0xaf12088595c0d5615f1e9da6033a435705d60037c2a4230dd5e135aab9dcfeb5", 10],
  ["CAKE pancakeswap_v2 0x570e2162d9ba3000a088ea1680e21616d177b7c0", "v2", "0x570e2162d9ba3000a088ea1680e21616d177b7c0", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["CAKE pancakeswap_v2 0xa458ed5d77eb13ccae0ac237f7bb7ca3759fa6c3", "v2", "0xa458ed5d77eb13ccae0ac237f7bb7ca3759fa6c3", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["ETH pancakeswap-v3-bsc 0xd0e226f674bbf064f54ab47f42473ff80db98cba", "pcs_v3", "0xd0e226f674bbf064f54ab47f42473ff80db98cba", "0x0000000000000000000000000000000000000000000000000000000000000000", 10],
  ["ETH pancakeswap-v3-bsc 0x62fcb3c1794fb95bd8b1a97f6ad5d8a7e4943a1e", "pcs_v3", "0x62fcb3c1794fb95bd8b1a97f6ad5d8a7e4943a1e", "0x0000000000000000000000000000000000000000000000000000000000000000", 1],
  ["ETH pancakeswap-v3-bsc 0xbe141893e4c6ad9272e8c04bab7e6a10604501a5", "pcs_v3", "0xbe141893e4c6ad9272e8c04bab7e6a10604501a5", "0x0000000000000000000000000000000000000000000000000000000000000000", 10],
  ["ETH uniswap-bsc 0x0f338ec12d3f7c3d77a4b9fcc1f95f3fb6ad0ea6", "uni_v3", "0x0f338ec12d3f7c3d77a4b9fcc1f95f3fb6ad0ea6", "0x0000000000000000000000000000000000000000000000000000000000000000", 10],
  ["ETH pancakeswap-v3-bsc 0x4bba1018b967e59220b22ca03f68821a3276c9a6", "pcs_v3", "0x4bba1018b967e59220b22ca03f68821a3276c9a6", "0x0000000000000000000000000000000000000000000000000000000000000000", 10],
  ["ETH pancakeswap-v3-bsc 0x9f599f3d64a9d99ea21e68127bb6ce99f893da61", "pcs_v3", "0x9f599f3d64a9d99ea21e68127bb6ce99f893da61", "0x0000000000000000000000000000000000000000000000000000000000000000", 1],
  ["ETH pancakeswap_v2 0x74e4716e431f45807dcf19f284c7aa99f18a4fbc", "v2", "0x74e4716e431f45807dcf19f284c7aa99f18a4fbc", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["ETH uniswap-v4-bsc 0xd02643e9323895f4b562c92f3e6ced7e77db73cd8ab057ec685d1dc710551a6b", "uni_v4", "0x0000000000000000000000000000000000000000", "0xd02643e9323895f4b562c92f3e6ced7e77db73cd8ab057ec685d1dc710551a6b", 25],
  ["ETH uniswap-bsc 0x3fb2623567e21f8c50f0ae86f54ef4849b4eb47b", "uni_v3", "0x3fb2623567e21f8c50f0ae86f54ef4849b4eb47b", "0x0000000000000000000000000000000000000000000000000000000000000000", 60],
  ["ETH pancakeswap_v2 0x272c9c291a24349c5cc79ac10d6d4c9fb780b950", "v2", "0x272c9c291a24349c5cc79ac10d6d4c9fb780b950", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["ETH uniswap-bsc 0x0416e8dd7d165f5e5af5065abfe6accc2945d58a", "uni_v3", "0x0416e8dd7d165f5e5af5065abfe6accc2945d58a", "0x0000000000000000000000000000000000000000000000000000000000000000", 60],
  ["ETH pancakeswap_v2 0x531febfeb9a61d948c384acfbe6dcc51057aea7e", "v2", "0x531febfeb9a61d948c384acfbe6dcc51057aea7e", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["ETH pancakeswap_v2 0x7fbef458f5c980a1701716a73c480e9b3e9bef38", "v2", "0x7fbef458f5c980a1701716a73c480e9b3e9bef38", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["ETH uniswap-bsc 0xf9878a5dd55edc120fde01893ea713a4f032229c", "uni_v3", "0xf9878a5dd55edc120fde01893ea713a4f032229c", "0x0000000000000000000000000000000000000000000000000000000000000000", 10],
  ["ETH pancakeswap_v2 0xa06ba58ce2a79fdd92345fd96050c0ad1a030007", "v2", "0xa06ba58ce2a79fdd92345fd96050c0ad1a030007", "0x0000000000000000000000000000000000000000000000000000000000000000", 0],
  ["ETH pancakeswap-v3-bsc 0x539e0ebfffd39e54a0f7e5f8fec40ade7933a664", "pcs_v3", "0x539e0ebfffd39e54a0f7e5f8fec40ade7933a664", "0x0000000000000000000000000000000000000000000000000000000000000000", 10]
  ];
  // Ступень 1: [имя, контракт, данные]
  const STAGE1 = [
  ["s1 CAKE pancakeswap_v2 0x0ed7e52944161450477ee417de9cd3a859b14fd0 getReserves", "0x0ed7e52944161450477ee417de9cd3a859b14fd0", "0x0902f1ac"],
  ["s1 CAKE pancakeswap-v3-bsc 0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1 slot0", "0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1", "0x3850c7bd"],
  ["s1 CAKE pancakeswap-v3-bsc 0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1 liquidity", "0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1", "0x1a686502"],
  ["s1 CAKE pancakeswap-v3-bsc 0x133b3d95bad5405d14d53473671200e9342896bf slot0", "0x133b3d95bad5405d14d53473671200e9342896bf", "0x3850c7bd"],
  ["s1 CAKE pancakeswap-v3-bsc 0x133b3d95bad5405d14d53473671200e9342896bf liquidity", "0x133b3d95bad5405d14d53473671200e9342896bf", "0x1a686502"],
  ["s1 CAKE pancakeswap-v3-bsc 0xafb2da14056725e3ba3a30dd846b6bbbd7886c56 slot0", "0xafb2da14056725e3ba3a30dd846b6bbbd7886c56", "0x3850c7bd"],
  ["s1 CAKE pancakeswap-v3-bsc 0xafb2da14056725e3ba3a30dd846b6bbbd7886c56 liquidity", "0xafb2da14056725e3ba3a30dd846b6bbbd7886c56", "0x1a686502"],
  ["s1 CAKE uniswap-bsc 0xfe4fe5b4575c036ac6d5cccfe13660020270e27a slot0", "0xfe4fe5b4575c036ac6d5cccfe13660020270e27a", "0x3850c7bd"],
  ["s1 CAKE uniswap-bsc 0xfe4fe5b4575c036ac6d5cccfe13660020270e27a liquidity", "0xfe4fe5b4575c036ac6d5cccfe13660020270e27a", "0x1a686502"],
  ["s1 CAKE pancakeswap-infinity-clmm 0x54c27041dfa246727d9351613eb35da028ddf377225d8db9e68ca3b569b5ba24 CLPoolManager getSlot0", "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b", "0xc815641c54c27041dfa246727d9351613eb35da028ddf377225d8db9e68ca3b569b5ba24"],
  ["s1 CAKE pancakeswap-infinity-clmm 0x54c27041dfa246727d9351613eb35da028ddf377225d8db9e68ca3b569b5ba24 CLPoolManager getLiquidity", "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b", "0xfa6793d554c27041dfa246727d9351613eb35da028ddf377225d8db9e68ca3b569b5ba24"],
  ["s1 CAKE pancakeswap-infinity-clmm 0x47516855520496b84a169f7bb92ace7ffb6e8c535bccb52a308ccff113aeccfb CLPoolManager getSlot0", "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b", "0xc815641c47516855520496b84a169f7bb92ace7ffb6e8c535bccb52a308ccff113aeccfb"],
  ["s1 CAKE pancakeswap-infinity-clmm 0x47516855520496b84a169f7bb92ace7ffb6e8c535bccb52a308ccff113aeccfb CLPoolManager getLiquidity", "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b", "0xfa6793d547516855520496b84a169f7bb92ace7ffb6e8c535bccb52a308ccff113aeccfb"],
  ["s1 CAKE pancakeswap_v2 0xde52cff316d8a70256bee647073312c1aa7ee2cf getReserves", "0xde52cff316d8a70256bee647073312c1aa7ee2cf", "0x0902f1ac"],
  ["s1 CAKE uniswap-v4-bsc 0x4630fb37706efaf996be1ee54b6841f8d9548ac42ae6cebc02511d891726d1e6 StateView getSlot0", "0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4", "0xc815641c4630fb37706efaf996be1ee54b6841f8d9548ac42ae6cebc02511d891726d1e6"],
  ["s1 CAKE uniswap-v4-bsc 0x4630fb37706efaf996be1ee54b6841f8d9548ac42ae6cebc02511d891726d1e6 StateView getLiquidity", "0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4", "0xfa6793d54630fb37706efaf996be1ee54b6841f8d9548ac42ae6cebc02511d891726d1e6"],
  ["s1 CAKE pancakeswap_v2 0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b getReserves", "0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b", "0x0902f1ac"],
  ["s1 CAKE pancakeswap-v3-bsc 0x8ec186cd1ad51c380bd23fde29f852226647616c slot0", "0x8ec186cd1ad51c380bd23fde29f852226647616c", "0x3850c7bd"],
  ["s1 CAKE pancakeswap-v3-bsc 0x8ec186cd1ad51c380bd23fde29f852226647616c liquidity", "0x8ec186cd1ad51c380bd23fde29f852226647616c", "0x1a686502"],
  ["s1 CAKE pancakeswap_v2 0x804678fa97d91b974ec2af3c843270886528a9e6 getReserves", "0x804678fa97d91b974ec2af3c843270886528a9e6", "0x0902f1ac"],
  ["s1 CAKE uniswap-bsc 0x8e8410f4878079fc03a287a7ded4a65afb87cc06 slot0", "0x8e8410f4878079fc03a287a7ded4a65afb87cc06", "0x3850c7bd"],
  ["s1 CAKE uniswap-bsc 0x8e8410f4878079fc03a287a7ded4a65afb87cc06 liquidity", "0x8e8410f4878079fc03a287a7ded4a65afb87cc06", "0x1a686502"],
  ["s1 CAKE pancakeswap-infinity-clmm 0x737a7d974a19bafb34c8d74d898188c9b59689b91f291fa6ade69f71fa0f5afa CLPoolManager getSlot0", "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b", "0xc815641c737a7d974a19bafb34c8d74d898188c9b59689b91f291fa6ade69f71fa0f5afa"],
  ["s1 CAKE pancakeswap-infinity-clmm 0x737a7d974a19bafb34c8d74d898188c9b59689b91f291fa6ade69f71fa0f5afa CLPoolManager getLiquidity", "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b", "0xfa6793d5737a7d974a19bafb34c8d74d898188c9b59689b91f291fa6ade69f71fa0f5afa"],
  ["s1 CAKE pancakeswap_v2 0x5ffc1857a39a8d63e556e988f8e28ce6cf7d0db9 getReserves", "0x5ffc1857a39a8d63e556e988f8e28ce6cf7d0db9", "0x0902f1ac"],
  ["s1 CAKE pancakeswap-v3-bsc 0xeb7528398b2725e1e0374734a87320132223a5c6 slot0", "0xeb7528398b2725e1e0374734a87320132223a5c6", "0x3850c7bd"],
  ["s1 CAKE pancakeswap-v3-bsc 0xeb7528398b2725e1e0374734a87320132223a5c6 liquidity", "0xeb7528398b2725e1e0374734a87320132223a5c6", "0x1a686502"],
  ["s1 CAKE pancakeswap-infinity-clmm 0xaf12088595c0d5615f1e9da6033a435705d60037c2a4230dd5e135aab9dcfeb5 CLPoolManager getSlot0", "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b", "0xc815641caf12088595c0d5615f1e9da6033a435705d60037c2a4230dd5e135aab9dcfeb5"],
  ["s1 CAKE pancakeswap-infinity-clmm 0xaf12088595c0d5615f1e9da6033a435705d60037c2a4230dd5e135aab9dcfeb5 CLPoolManager getLiquidity", "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b", "0xfa6793d5af12088595c0d5615f1e9da6033a435705d60037c2a4230dd5e135aab9dcfeb5"],
  ["s1 CAKE pancakeswap_v2 0x570e2162d9ba3000a088ea1680e21616d177b7c0 getReserves", "0x570e2162d9ba3000a088ea1680e21616d177b7c0", "0x0902f1ac"],
  ["s1 CAKE pancakeswap_v2 0xa458ed5d77eb13ccae0ac237f7bb7ca3759fa6c3 getReserves", "0xa458ed5d77eb13ccae0ac237f7bb7ca3759fa6c3", "0x0902f1ac"],
  ["s1 ETH pancakeswap-v3-bsc 0xd0e226f674bbf064f54ab47f42473ff80db98cba slot0", "0xd0e226f674bbf064f54ab47f42473ff80db98cba", "0x3850c7bd"],
  ["s1 ETH pancakeswap-v3-bsc 0xd0e226f674bbf064f54ab47f42473ff80db98cba liquidity", "0xd0e226f674bbf064f54ab47f42473ff80db98cba", "0x1a686502"],
  ["s1 ETH pancakeswap-v3-bsc 0x62fcb3c1794fb95bd8b1a97f6ad5d8a7e4943a1e slot0", "0x62fcb3c1794fb95bd8b1a97f6ad5d8a7e4943a1e", "0x3850c7bd"],
  ["s1 ETH pancakeswap-v3-bsc 0x62fcb3c1794fb95bd8b1a97f6ad5d8a7e4943a1e liquidity", "0x62fcb3c1794fb95bd8b1a97f6ad5d8a7e4943a1e", "0x1a686502"],
  ["s1 ETH pancakeswap-v3-bsc 0xbe141893e4c6ad9272e8c04bab7e6a10604501a5 slot0", "0xbe141893e4c6ad9272e8c04bab7e6a10604501a5", "0x3850c7bd"],
  ["s1 ETH pancakeswap-v3-bsc 0xbe141893e4c6ad9272e8c04bab7e6a10604501a5 liquidity", "0xbe141893e4c6ad9272e8c04bab7e6a10604501a5", "0x1a686502"],
  ["s1 ETH uniswap-bsc 0x0f338ec12d3f7c3d77a4b9fcc1f95f3fb6ad0ea6 slot0", "0x0f338ec12d3f7c3d77a4b9fcc1f95f3fb6ad0ea6", "0x3850c7bd"],
  ["s1 ETH uniswap-bsc 0x0f338ec12d3f7c3d77a4b9fcc1f95f3fb6ad0ea6 liquidity", "0x0f338ec12d3f7c3d77a4b9fcc1f95f3fb6ad0ea6", "0x1a686502"],
  ["s1 ETH pancakeswap-v3-bsc 0x4bba1018b967e59220b22ca03f68821a3276c9a6 slot0", "0x4bba1018b967e59220b22ca03f68821a3276c9a6", "0x3850c7bd"],
  ["s1 ETH pancakeswap-v3-bsc 0x4bba1018b967e59220b22ca03f68821a3276c9a6 liquidity", "0x4bba1018b967e59220b22ca03f68821a3276c9a6", "0x1a686502"],
  ["s1 ETH pancakeswap-v3-bsc 0x9f599f3d64a9d99ea21e68127bb6ce99f893da61 slot0", "0x9f599f3d64a9d99ea21e68127bb6ce99f893da61", "0x3850c7bd"],
  ["s1 ETH pancakeswap-v3-bsc 0x9f599f3d64a9d99ea21e68127bb6ce99f893da61 liquidity", "0x9f599f3d64a9d99ea21e68127bb6ce99f893da61", "0x1a686502"],
  ["s1 ETH pancakeswap_v2 0x74e4716e431f45807dcf19f284c7aa99f18a4fbc getReserves", "0x74e4716e431f45807dcf19f284c7aa99f18a4fbc", "0x0902f1ac"],
  ["s1 ETH uniswap-v4-bsc 0xd02643e9323895f4b562c92f3e6ced7e77db73cd8ab057ec685d1dc710551a6b StateView getSlot0", "0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4", "0xc815641cd02643e9323895f4b562c92f3e6ced7e77db73cd8ab057ec685d1dc710551a6b"],
  ["s1 ETH uniswap-v4-bsc 0xd02643e9323895f4b562c92f3e6ced7e77db73cd8ab057ec685d1dc710551a6b StateView getLiquidity", "0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4", "0xfa6793d5d02643e9323895f4b562c92f3e6ced7e77db73cd8ab057ec685d1dc710551a6b"],
  ["s1 ETH uniswap-bsc 0x3fb2623567e21f8c50f0ae86f54ef4849b4eb47b slot0", "0x3fb2623567e21f8c50f0ae86f54ef4849b4eb47b", "0x3850c7bd"],
  ["s1 ETH uniswap-bsc 0x3fb2623567e21f8c50f0ae86f54ef4849b4eb47b liquidity", "0x3fb2623567e21f8c50f0ae86f54ef4849b4eb47b", "0x1a686502"],
  ["s1 ETH pancakeswap_v2 0x272c9c291a24349c5cc79ac10d6d4c9fb780b950 getReserves", "0x272c9c291a24349c5cc79ac10d6d4c9fb780b950", "0x0902f1ac"],
  ["s1 ETH uniswap-bsc 0x0416e8dd7d165f5e5af5065abfe6accc2945d58a slot0", "0x0416e8dd7d165f5e5af5065abfe6accc2945d58a", "0x3850c7bd"],
  ["s1 ETH uniswap-bsc 0x0416e8dd7d165f5e5af5065abfe6accc2945d58a liquidity", "0x0416e8dd7d165f5e5af5065abfe6accc2945d58a", "0x1a686502"],
  ["s1 ETH pancakeswap_v2 0x531febfeb9a61d948c384acfbe6dcc51057aea7e getReserves", "0x531febfeb9a61d948c384acfbe6dcc51057aea7e", "0x0902f1ac"],
  ["s1 ETH pancakeswap_v2 0x7fbef458f5c980a1701716a73c480e9b3e9bef38 getReserves", "0x7fbef458f5c980a1701716a73c480e9b3e9bef38", "0x0902f1ac"],
  ["s1 ETH uniswap-bsc 0xf9878a5dd55edc120fde01893ea713a4f032229c slot0", "0xf9878a5dd55edc120fde01893ea713a4f032229c", "0x3850c7bd"],
  ["s1 ETH uniswap-bsc 0xf9878a5dd55edc120fde01893ea713a4f032229c liquidity", "0xf9878a5dd55edc120fde01893ea713a4f032229c", "0x1a686502"],
  ["s1 ETH pancakeswap_v2 0xa06ba58ce2a79fdd92345fd96050c0ad1a030007 getReserves", "0xa06ba58ce2a79fdd92345fd96050c0ad1a030007", "0x0902f1ac"],
  ["s1 ETH pancakeswap-v3-bsc 0x539e0ebfffd39e54a0f7e5f8fec40ade7933a664 slot0", "0x539e0ebfffd39e54a0f7e5f8fec40ade7933a664", "0x3850c7bd"],
  ["s1 ETH pancakeswap-v3-bsc 0x539e0ebfffd39e54a0f7e5f8fec40ade7933a664 liquidity", "0x539e0ebfffd39e54a0f7e5f8fec40ade7933a664", "0x1a686502"]
  ];

  const MASK = (1n << 256n) - 1n;
  const strip = (h) => h.replace(/^0x/, '').toLowerCase();
  const word = (h) => strip(h).padStart(64, '0');
  const int256 = (n) => (BigInt(n) & MASK).toString(16).padStart(64, '0');
  const signed = (h) => {
    const v = BigInt('0x' + h);
    return v >= 1n << 255n ? v - (1n << 256n) : v;
  };
  const floorDiv = (a, b) => Math.floor(a / b);

  let seq = 0;
  async function rpc(method, params) {
    const r = await fetch(NODE, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', id: ++seq, method, params }),
    });
    return r.json();
  }

  const results = [];
  const got = {};
  const head = { name: 'block', method: 'eth_blockNumber', params: [] };
  const first = await rpc('eth_blockNumber', []);
  const block = first.result;
  results.push({ name: 'block', request: head, response: first });
  console.log('блок', block, parseInt(block, 16));

  // Вызовы списком [имя, контракт, данные] — отдельными eth_call на закреплённом блоке.
  async function each(list) {
    const out = new Array(list.length);
    let next = 0;
    async function worker() {
      while (next < list.length) {
        const i = next++;
        const [name, to, input] = list[i];
        const request = { name, method: 'eth_call', params: [{ to, input }, block] };
        let response;
        try {
          response = await rpc('eth_call', request.params);
        } catch (e) {
          response = { error: { message: 'fetch: ' + e } };
        }
        if (typeof response.result === 'string') got[name] = strip(response.result);
        out[i] = { name, request, response };
      }
    }
    await Promise.all(Array.from({ length: PARALLEL }, worker));
    results.push(...out);
  }

  // Multicall3.aggregate3: массив (address target, bool allowFailure = true, bytes callData).
  function aggregate3(list) {
    const parts = list.map(([, to, input]) => {
      const d = strip(input);
      const len = d.length / 2;
      const padded = d.padEnd(Math.ceil(len / 32) * 64, '0');
      return word(to) + word('1') + word('60') + word(len.toString(16)) + padded;
    });
    let off = 32 * list.length;
    const offs = parts.map((p) => {
      const o = off;
      off += p.length / 2;
      return word(o.toString(16));
    });
    return '0x' + SEL.aggregate3 + word('20') + word(list.length.toString(16)) + offs.join('') + parts.join('');
  }

  // Имя первого вызова ступени 1 пула (ответ с тиком), как head() в Rust.
  const slot0Name = ([tag, kind]) =>
    kind === 'infinity_cl'
      ? `s1 ${tag} CLPoolManager getSlot0`
      : kind === 'uni_v4'
        ? `s1 ${tag} StateView getSlot0`
        : `s1 ${tag} slot0`;

  // Ступень 2 пула: слова [lo, hi].
  function stage2([tag, kind, addr, id], lo, hi) {
    const out = [];
    for (let w = lo; w <= hi; w++) {
      if (kind === 'pcs_v3' || kind === 'uni_v3') {
        const lens = kind === 'pcs_v3' ? PCS_TICK_LENS : UNI_TICK_LENS;
        out.push([`s2 ${tag} TickLens word ${w}`, lens, '0x' + SEL.lens + word(addr) + int256(w)]);
      } else if (kind === 'infinity_cl') {
        out.push([`s2 ${tag} CLPoolManager getPoolBitmapInfo ${w}`, CL_MANAGER, '0x' + SEL.bitmapInf + word(id) + int256(w)]);
      } else {
        out.push([`s2 ${tag} StateView getTickBitmap ${w}`, STATE_VIEW, '0x' + SEL.bitmapV4 + word(id) + int256(w)]);
      }
    }
    return out;
  }

  // Ступень 3 пула CL: тики.
  function stage3([tag, kind, , id], ticks) {
    return ticks.map((t) =>
      kind === 'infinity_cl'
        ? [`s3 ${tag} CLPoolManager getPoolTickInfo ${t}`, CL_MANAGER, '0x' + SEL.tickInf + word(id) + int256(t)]
        : [`s3 ${tag} StateView getTickLiquidity ${t}`, STATE_VIEW, '0x' + SEL.tickV4 + word(id) + int256(t)],
    );
  }

  // Ступень 1.
  await each(STAGE1);

  // Ступень 2: тик — второе слово ответа slot0 / getSlot0 (int24 в дополнительном коде).
  const ranges = {};
  const s2 = [];
  for (const p of POOLS) {
    if (p[1] === 'v2') continue;
    const r = got[slot0Name(p)];
    if (!r || r.length < 128) continue;
    const tick = Number(signed(r.slice(64, 128)));
    const spacing = p[4];
    const w = floorDiv(floorDiv(tick, spacing), 256);
    const lo = Math.max(w - TICK_WORDS, -32768);
    const hi = Math.min(w + TICK_WORDS, 32767);
    ranges[p[0]] = [lo, hi];
    s2.push(...stage2(p, lo, hi));
  }
  await each(s2);

  // Ступень 3: установленные биты слов CL (по словам, затем по битам — как load_cl::set_ticks).
  const s3 = [];
  for (const p of POOLS) {
    if (p[1] !== 'infinity_cl' && p[1] !== 'uni_v4') continue;
    const r = ranges[p[0]];
    if (!r) continue;
    const calls = stage2(p, r[0], r[1]);
    if (!calls.every(([n]) => got[n])) continue;
    const ticks = [];
    calls.forEach(([n], i) => {
      const v = BigInt('0x' + (got[n] || '0'));
      for (let b = 0; b < 256; b++) {
        if ((v >> BigInt(b)) & 1n) ticks.push(((r[0] + i) * 256 + b) * p[4]);
      }
    });
    s3.push(...stage3(p, ticks));
  }
  await each(s3);

  // Пачки aggregate3 ступеней 1–3, как #47: вызовы ступени подряд, части по BATCH.
  const stages = [STAGE1, s2, s3];
  const aggs = [];
  stages.forEach((list, k) => {
    for (let i = 0; i * BATCH < list.length; i++) {
      const part = list.slice(i * BATCH, (i + 1) * BATCH);
      aggs.push([`s${k + 1} Multicall3 aggregate3 часть ${i} (${part.length} вызовов)`, MULTICALL3, aggregate3(part)]);
    }
  });
  await each(aggs);

  const errors = results.filter((x) => x.response && x.response.error).length;
  const result = {
    about: `Живые ответы узла для пачечного чтения состояния пулов (#47), блок ${block}, dex.tick_words = ${TICK_WORDS}, dex.verify_batch = ${BATCH}. Записано tests/recorders/record_node_calls_3.js в браузере (${NODE}). s1/s2/s3 — ступени; каждый вызов отдельно и теми же пачками Multicall3.aggregate3.`,
    block,
    results,
  };
  window.__rec = result;
  console.log(`готово: вызовов ${results.length}, ступень 1 — ${STAGE1.length}, 2 — ${s2.length}, 3 — ${s3.length}, пачек ${aggs.length}, ошибок ${errors}; итог — window.__rec`);
})();
