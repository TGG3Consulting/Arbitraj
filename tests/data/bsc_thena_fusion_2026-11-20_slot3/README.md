# Источник записи (решение 463, T463-02 — повтор записи со слотом 3)

- Узел: свой узел BSC (Geth/v1.8.2-54afaff4-20261103, chainId 56 — meta.json), 60 пулов GeckoTerminal «thena-fusion»,
  каждый на своём блоке 134760116–134760122.
- Скрипт: tools/bsc_record/thena_fusion.py — коммит 45da3813 (T237-02: slot 3 пула, точки оракула index + 2 и 0, начало
  окна бота oracle.starts) с правкой 06a2c160 (время начала окна в ts1; без неё официальный Quoter давал revert).
- Котировки: 60 пулов × 8 × 4 времени = 1920, у каждой — виртуальный квотер бота и официальный Quoter THENA
  (0xeA68020D6A9532EeC42D4dB0f92B83580c39b2cA); все 3840 ответов без ошибок.
- Архив: bsc_thena_fusion_2026-11-20.zip, sha256 bccefd09acba0051252e8422f5b40a0ab89111885d92899204a711e51316b033 (по
  сообщению координатора; архива в репозитории нет). Папка переименована в bsc_thena_fusion_2026-11-20_slot3, чтобы не
  затереть первую запись 20.11 (tests/data/bsc_thena_fusion_2026-11-20). Предыдущая попытка этой записи (1920 revert
  Quoter, до 06a2c160) не используется.
- Тест: tests/bsc_thena_fusion_fee_slot3.rs.

# THENA Fusion — запись своего узла BSC (bsc_thena_fusion_2026-11-20)

Записано tools/bsc_record/thena_fusion.py (только чтение). Состав файлов — шапка скрипта.

- chainId: 56
- фабрика 0x306F06C147f064A010530292A1EB6737c3e378e4, poolDeployer 0xc89f69baa3ff17a842ab2de89e5fc8a8e2cc7358, keccak кода деплоера 0xece75c47cc9740f43b4cd0fc4e49c8c0d1ab2f18eee2892181d5385c0e95926f

- пул 0x1b9a1120a17617d8ec4dc80b921a9a1c50caef7d: блок 134760116, tickSpacing 60, keccak кода 0xf6f108013c1b964022a10d8a78d85551bb2cab4bd9e77767121dbbd749ba7ee2, CREATE2 по хешу ни одного; сырое толкование: битов 31, из них liquidityTotal > 0 — 1; сжатое: битов 31, liquidityTotal > 0 — 31
- пул 0x1123e75b71019962cd4d21b0f3018a6412edb63c: блок 134760116, tickSpacing 60, keccak кода 0x4f86d61189ec6e0231d300d9dd9f7746afb4a9205bd6ed2a8ca1c56ff34ae8eb, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 100, liquidityTotal > 0 — 100
- пул 0x6b67112aa7b45e8cdc0a93b8d66a6a36e68ae8e5: блок 134760116, tickSpacing 60, keccak кода 0x3fe40c7985208b2c038f2e36bf361af5a2b5d02abd8277b1fbb2889049e4340f, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 75, liquidityTotal > 0 — 75
- пул 0xd405b976ac01023c9064024880999fc450a8668b: блок 134760116, tickSpacing 60, keccak кода 0x25bfba3b11204bf12f8b72917a5ac6d5dac6b0b71255bac6345f45ac059dd193, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 80, liquidityTotal > 0 — 80
- пул 0x17661065fbc18e782a259db2022bc23bc74bd942: блок 134760116, tickSpacing 60, keccak кода 0x98ce261b8c5771600ef2c65b8c34f46881326630f8b779322fd0b07b34cd8dd4, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 29, liquidityTotal > 0 — 29
- пул 0x1b932598da273069e1878e4a1505b6f833462210: блок 134760116, tickSpacing 60, keccak кода 0x5d36613c140f6d41dc815a23e502329ac71df35d55d7c7f2d48522bc6f0b92a8, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 17, liquidityTotal > 0 — 17
- пул 0x752328a1e16d38933789860b8c09f6f2cc6c63d6: блок 134760117, tickSpacing 60, keccak кода 0x5518376721242ad6722042033c434191e359a812d2fece4a9871b7f2ba528c10, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 10, liquidityTotal > 0 — 10
- пул 0xa60a504d92a1c95bda729c3f745b361ca822d6dd: блок 134760117, tickSpacing 60, keccak кода 0xc1fd0682090f5b7147339e40cd71c86b5dc24832fe331edfb6657a56589fae3e, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 59, liquidityTotal > 0 — 59
- пул 0x246505db95e5a60d8524d52b9ed3dbaf6ee2584f: блок 134760117, tickSpacing 60, keccak кода 0x5e1ce34b5b5681eb381d63d5c8f32873687a6576a22c3d854a8d9c69271db131, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 8, liquidityTotal > 0 — 8
- пул 0x98a0004b8e9fe161369528a2e07de56c15a27d76: блок 134760117, tickSpacing 60, keccak кода 0x588c0e90197eff0e0ddcacbb7fbfc8d220097fb0b0f5442669ee551feaec743b, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 44, liquidityTotal > 0 — 44
- пул 0x51bd5e6d3da9064d59bcaa5a76776560ab42ceb8: блок 134760117, tickSpacing 60, keccak кода 0xb588edba48994b007d8289ba9ef00499f6b5f5372c4376e9042c2e92a9990794, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 109, liquidityTotal > 0 — 109
- пул 0x1f9b1a3ddedbf47b96c65f29c0586b678de2623b: блок 134760117, tickSpacing 60, keccak кода 0xdacd59d6a39a9af42f77a92bc85db4d92c365b92f24039bbb50e41ebe84c22eb, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 10, liquidityTotal > 0 — 10
- пул 0x7c0b5d39765b221810d477e8f02d47a9badf018a: блок 134760117, tickSpacing 60, keccak кода 0xd1ebc17207684bb06af8657cc5f9807deea42d0be12dd86e9a6891d25fc377d6, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 14, liquidityTotal > 0 — 14
- пул 0xb3f3312252cade3a15eba318f6caaacb5e8097f4: блок 134760117, tickSpacing 60, keccak кода 0xd9e1b8582369785736e8a201180363e71468265ee6efc75989f865cc637bead5, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 6, liquidityTotal > 0 — 6
- пул 0x775cac1c1b47250121291934d15912efb69213e9: блок 134760117, tickSpacing 60, keccak кода 0xb82fc0ee3a405608aeb3a46fd8c272d81d241d3f24bb15df19ae6a5946db5960, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 17, liquidityTotal > 0 — 17
- пул 0x1a2a83deab03dc7110080e7f83df7ffe36681289: блок 134760118, tickSpacing 60, keccak кода 0x575e5c0d5e7dcd49de23928a1159977c6ecee014211984eb1d9429910f0a82e1, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 5, liquidityTotal > 0 — 5
- пул 0x44aa475ed44dda30f4fb81a0eec7c44aed01c7c0: блок 134760118, tickSpacing 60, keccak кода 0x0be3dd5da24f6e5e86d92f419f81b55a2b5ec672233b6ad0ad9a3007bfe2feab, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 26, liquidityTotal > 0 — 26
- пул 0xde045462392a86a313239c5cbac87f7f57a92402: блок 134760118, tickSpacing 60, keccak кода 0x88b7c500942f9a2e04b9fbe5af6d37d35663bb7acdc306a21c2e7716d559cf5b, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 4, liquidityTotal > 0 — 4
- пул 0xff4840a06708e53b620e2670295265029fa72a78: блок 134760118, tickSpacing 60, keccak кода 0x7c8840a96e39171f3b86a06d0fd57a8c65c50d93daea166f58ad0afc8841c34f, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 18, liquidityTotal > 0 — 18
- пул 0xc7db746edd16bbde9a75aa6114400709041da561: блок 134760118, tickSpacing 60, keccak кода 0x18eda62414260696d03cbf50f7436075f8a92ec447c4b15e6f7437f768d20eaf, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 2, liquidityTotal > 0 — 2
- пул 0xf2a4e4261fcdfbb891bcf703640fbe713c6cd0fe: блок 134760118, tickSpacing 60, keccak кода 0x37b4a50290bf4f2724fcd52d788ff39edf13e3d8137dd42df3db8800809bdaac, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 8, liquidityTotal > 0 — 8
- пул 0x52ddb7bd7ca73a20b6334cc08fcaca9906992a15: блок 134760118, tickSpacing 60, keccak кода 0x1cef4950bf194938cf65eed60c696c897f69c9838006ccbd685a4cec276f16e6, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 9, liquidityTotal > 0 — 9
- пул 0x1a0a28d2217503f29f5579c16d8783bd7b9c8c93: блок 134760118, tickSpacing 60, keccak кода 0xd1f614a45a7146e58a521658cb51d023933f8d6567c3b6f616d7a54a1ed843cd, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 6, liquidityTotal > 0 — 6
- пул 0x6eb8a743723216a0de043c554c85e447b0d59e2e: блок 134760119, tickSpacing 60, keccak кода 0x7716d50488cb6b5ade6565df2bff57f9aab045130226c04ba019d2ec27b0cfd5, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 4, liquidityTotal > 0 — 4
- пул 0xbf5e6e3c88c88cf6b17327f57a61a3a997f13a3b: блок 134760119, tickSpacing 60, keccak кода 0xd5d184007f437859857d44b340f80b36a712f39162bbde72b582d029e0d9b4c0, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 8, liquidityTotal > 0 — 8
- пул 0x8ab8fd902039a48e51735452b1f97c93cb034e80: блок 134760119, tickSpacing 60, keccak кода 0x1728f8ed6f9262af97ba56ca69804e08433cf15f809636b2cb5ac5c364364822, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 10, liquidityTotal > 0 — 10
- пул 0x01e0d483ac9e04c443ad1b82f0e967813962d568: блок 134760119, tickSpacing 60, keccak кода 0xa90f77ec7a5195df997229af12c0d6c658677ea990190894795ee6865033b6c2, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 8, liquidityTotal > 0 — 8
- пул 0xf860e573fd87669e9004c108cd8245962f96b444: блок 134760119, tickSpacing 60, keccak кода 0xd3df13cfaf450c1806f3f2108ead88afa1cb2983f67c95993a24c54e70ad27b1, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 8, liquidityTotal > 0 — 8
- пул 0x5135c813d5f9d3624221b3bdbac805339fa5a591: блок 134760119, tickSpacing 60, keccak кода 0xaedcae62a003572b68b717bae03f2224c12d9596891b55410dcafb88b64c3845, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 7, liquidityTotal > 0 — 7
- пул 0x755a52d29b24d6871899a84f476339183e9dc95d: блок 134760119, tickSpacing 60, keccak кода 0x6a23f6d27c638a4b86581a410e3d0b8c092acaf772ebbd461d6edd301498eb1e, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 10, liquidityTotal > 0 — 10
- пул 0x5b0baf66718caabda49a4af32eb455c3b99b5821: блок 134760119, tickSpacing 60, keccak кода 0x5ca8e417d6d553c62fe5398dc956601f1ac2212a755abcdacefef0646afddfa8, CREATE2 по хешу ни одного; сырое толкование: битов 8, из них liquidityTotal > 0 — 1; сжатое: битов 8, liquidityTotal > 0 — 8
- пул 0xbf121d987f9635ed6d2f7bb957fbbe163bdea0e0: блок 134760119, tickSpacing 60, keccak кода 0xb8cbf10ee6d16e95ff118953cafb5df33d7c3d4eead478529c7b1a398abcaa4d, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 11, liquidityTotal > 0 — 11
- пул 0xd33f3c50df98d3bf0f47d8848340874a3bf9362d: блок 134760120, tickSpacing 60, keccak кода 0x03b4e074538ac41d218d64191834b80a1acb847b2ca0bebe73bf66922b1e204c, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 0, liquidityTotal > 0 — 0
- пул 0xab6f06a33f38cba5a5312de24151cb91da2b0eb0: блок 134760120, tickSpacing 60, keccak кода 0xca6558300af44b76bc18f1bc0b251013f08face508c59c90a7788fc1e3723df0, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 8, liquidityTotal > 0 — 8
- пул 0xd6f0ba7ec72ff3974b02c18ed1fc33da77434d41: блок 134760120, tickSpacing 60, keccak кода 0x69db0918cf8050b7a33b2635a14fc90fd889bd8f6af1089d81f99bb6200d615f, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 14, liquidityTotal > 0 — 14
- пул 0xb0d6a73b6c9014f58546dcb376f527f3aa71c4d4: блок 134760120, tickSpacing 60, keccak кода 0x39766e0267dc9f148fed26a0cc4e2df690ce6ef4e032125baab7bf7ac6292b2d, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 4, liquidityTotal > 0 — 4
- пул 0xa07bbf09b48e8d219774ac9b92622f5260a9c9f4: блок 134760120, tickSpacing 60, keccak кода 0xe2510d763cd7921da144a6c15d3f39931f1c2e660f35508a51f45330fc972263, CREATE2 по хешу ни одного; сырое толкование: битов 7, из них liquidityTotal > 0 — 0; сжатое: битов 10, liquidityTotal > 0 — 10
- пул 0x9c487a70e3c2a81c5f1fb59251ab246ba857cbaf: блок 134760120, tickSpacing 60, keccak кода 0x71ecda88cf48257b540ccb06775729de3f9872fdac02c80d91b6fbe6aa3c7b06, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 4, liquidityTotal > 0 — 4
- пул 0x8731bb0202deb31a93b3178b674e51c3cc49b285: блок 134760120, tickSpacing 60, keccak кода 0xdfdb97ebf8e22d857fe24dd2bb3acd97d491fab59831489ccc2b715b492b6b7b, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 1, liquidityTotal > 0 — 1
- пул 0x2f6c6e00e517944ee5efe310cd0b98a3fc61cb98: блок 134760120, tickSpacing 60, keccak кода 0xb78e1134e61a80b763680e3249d2ede26e931c599fb5daadfdd1874d888ee1c4, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 13, liquidityTotal > 0 — 13
- пул 0x8e3ae967604dc4b4ba676e6d34ac603fb07fc921: блок 134760120, tickSpacing 60, keccak кода 0xdc9fe8e20fedeffdcccf89769ae0d199f8efb0c61e8b6fc669f0ae3d8ad2ce93, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 5, liquidityTotal > 0 — 5
- пул 0xc57061da1894ae58fb834f6db33e9a45cc4e7807: блок 134760121, tickSpacing 60, keccak кода 0x151e871b76bc84f4db1dca67b3b987a0973e57c0c7394ad2bb6777e4bbd6a3fd, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 7, liquidityTotal > 0 — 7
- пул 0xc1716bea36140fa08476742d80cb28cecd492a6f: блок 134760121, tickSpacing 60, keccak кода 0xc20200e05ed693af6ea72fbd4070caf31650ce0085fc248c5fd3b0ba815e3572, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 1, liquidityTotal > 0 — 1
- пул 0xd688108301df5d939bf272eb378429488a72ea52: блок 134760121, tickSpacing 60, keccak кода 0xb0dc8fae64fc81366a5582d9880d74998313158e64e11f01ae24c2f0f137e868, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 1, liquidityTotal > 0 — 1
- пул 0x11c92ded470a50c96d2e5effedc890cd4bd2502e: блок 134760121, tickSpacing 60, keccak кода 0x9621233137322e96ac3ba0f3b5921b195e6293792c26cf9ba3bab1cddbff4a62, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 1, liquidityTotal > 0 — 1
- пул 0x0d954c467a58c512b5faa2c9071f5143d10f7171: блок 134760121, tickSpacing 60, keccak кода 0x04c804354ae5928c7d646695b4e94d28e5c7532df023b86818f2c844472318ac, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 4, liquidityTotal > 0 — 4
- пул 0x9d49003383ab126c563ae2b7d545db003e657f53: блок 134760121, tickSpacing 60, keccak кода 0x33d9dc150f0680442a82290a8f493db59a9a8ad98fa7722061b3a15d69fbae11, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 2, liquidityTotal > 0 — 2
- пул 0x3a8b7e833793ab906403238853db34bd3231508d: блок 134760121, tickSpacing 60, keccak кода 0x91515ac0f04658221ec67e7c3c2e933c5e5a411905295aeb0992b63a56f6809b, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 1, liquidityTotal > 0 — 1
- пул 0x489bf777382306bc7de514fa0f1a4c725aebe98f: блок 134760121, tickSpacing 60, keccak кода 0x1dd9f00b74d5c9f02375befa1ae42130beee4e484d1fbf7721b0a86b930923c1, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 0, liquidityTotal > 0 — 0
- пул 0x117e96887b20d4db40df85fa2b7302ca0d63c5e3: блок 134760121, tickSpacing 60, keccak кода 0x71e7fede1be926e60d2083c073891c3d206ecb9ed0a41f3430852369fc13d4a9, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 10, liquidityTotal > 0 — 10
- пул 0x055557c6606f7b0d34e617653c447f079b0b0a73: блок 134760122, tickSpacing 60, keccak кода 0x83e858f128f43e7fbcf758936f1d06352dc3e3e19a927cdebdeb677272883f6f, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 1, liquidityTotal > 0 — 1
- пул 0xf6814792050ccd1c603d8a95468d6d9587ecbdaf: блок 134760122, tickSpacing 60, keccak кода 0xf296027edbfeebf3c073f393342a209a6c8313e24d13beabd1a572c5ca95c306, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 20, liquidityTotal > 0 — 20
- пул 0x06dc242b1987b46c1e8dd44d1cb203139cf09edf: блок 134760122, tickSpacing 60, keccak кода 0xec96c2a34da034b5d0a85ec8cdbda12ca1b4686ddc425bb40149bbf260b093f4, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 6, liquidityTotal > 0 — 6
- пул 0x28b23d6ad0e440a37994eb49dc0b62dbcdea99a9: блок 134760122, tickSpacing 60, keccak кода 0x364014dbb3331ce3c3692746e36050f8e0bb28ef6670ef39746753b80d35e263, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 8, liquidityTotal > 0 — 8
- пул 0x158480ed99b4b6b917acaffbfaa00546d25b50a6: блок 134760122, tickSpacing 60, keccak кода 0xf3c37a96646bdc86b8c2c2180a9ed5307c3513f0f81b13a913263695c57c9f3f, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 9, liquidityTotal > 0 — 9
- пул 0x987c794c0786ee5cd6b34b7e32aa21098cd6b806: блок 134760122, tickSpacing 60, keccak кода 0x72cc53580f86af250aef464f06499142e890c00782e9fa968d21d4e24a0cd28d, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 6, liquidityTotal > 0 — 6
- пул 0xc9df4ae113fa7cb67ce1779cb1560d1f43c2d373: блок 134760122, tickSpacing 60, keccak кода 0xcaca45c8cbe20af45325232325b282f5c4d223243cfa81efcbd1976fdf4d2ba5, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 3, liquidityTotal > 0 — 3
- пул 0x1335ffc90516b022425dbcfe4ea3610aa45f6c3e: блок 134760122, tickSpacing 60, keccak кода 0x07ebb62078881c5e501b91cad2468f3ff4d2d16017a798c786b4cba7a6438082, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 2, liquidityTotal > 0 — 2
- пул 0x059edc38a9ef11a64fbecc2c30c991ecca51d962: блок 134760122, tickSpacing 60, keccak кода 0x636e195b2923e068828e57db42ba5df0c86067a853cf6fe3a6be278db2a414df, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 6, liquidityTotal > 0 — 6
- пул 0x58b301e61705357cc78f815e604f9f9e486bc409: блок 134760122, tickSpacing 60, keccak кода 0x08cc68f1000644b8f8497e4ac7972be4164b7b46ec18b25f36f7cbdf10b3f6ad, CREATE2 по хешу ни одного; сырое толкование: битов 0, из них liquidityTotal > 0 — 0; сжатое: битов 2, liquidityTotal > 0 — 2

## SHA-256 файлов
16d0bb9b47338d4bc2c8565953effcb0462220bae501b605d896cf21291535f4  factory.json
f71bc9a3b6d0e739df7beba3d662ca43603e4db5d217130cc88d323687b80ce5  geckoterminal_pools.json
c377d0bb0ce197efb011608fccbd3520d27c716b838eb827896b9e9c1e9473f0  meta.json
98140d42842a3d5b5747e4cd7817236394b801d691090622e76243010e8d3a54  pool_0x01e0d483ac9e04c443ad1b82f0e967813962d568.json
722a6e042574e215561dbf42f2ab8a84e66673ad90ecf5d6fb3ff0086763dad8  pool_0x055557c6606f7b0d34e617653c447f079b0b0a73.json
ea23b5dff25831aa51b8f8360d1ebb8e2c95daa49d308837a08448ad0bb04654  pool_0x059edc38a9ef11a64fbecc2c30c991ecca51d962.json
2344ece408ba9d5247a93b794ad887a2e071e0cfe0c6a90568bdf00eb2a09271  pool_0x06dc242b1987b46c1e8dd44d1cb203139cf09edf.json
89bdfce7fe1bf60e299947cba9e03b1786dbc8fd413b07fc2ace6289da0cca57  pool_0x0d954c467a58c512b5faa2c9071f5143d10f7171.json
ef347c74de493789375177945bb94b661ae955a0d4718cc8933b10f86dd525b3  pool_0x1123e75b71019962cd4d21b0f3018a6412edb63c.json
511779a95d5d4bba799c2bee27e56ee87f55e251f1dabd06ef21fe4e4ac2c9f4  pool_0x117e96887b20d4db40df85fa2b7302ca0d63c5e3.json
74dbe1e3fd2129b2a74cb46ee48e174618d7944e3642c4d7c240bd8bf89d33a3  pool_0x11c92ded470a50c96d2e5effedc890cd4bd2502e.json
22a5ad848ef5e3fea6af2685bcd387f8beadeaa279b254354e99e5f7f49a80b2  pool_0x1335ffc90516b022425dbcfe4ea3610aa45f6c3e.json
5a0d95fc1a8290a5d58bbbb536217501db92635b8cfda8e5980df06bc56e3c73  pool_0x158480ed99b4b6b917acaffbfaa00546d25b50a6.json
9271e730a861725d35a235a0eb73a0e60e63c604b38afb7f34d21657a3f1bb3e  pool_0x17661065fbc18e782a259db2022bc23bc74bd942.json
1e5c4962f3900c2ebcedfc2c8d846d502ddc6fd5f91c881f308ac5f34416ba63  pool_0x1a0a28d2217503f29f5579c16d8783bd7b9c8c93.json
1cfb05e98847f2963d37947a92a6825b55a2d6abcfc54a18f59949b7c511ca99  pool_0x1a2a83deab03dc7110080e7f83df7ffe36681289.json
155f3c2b7bf7381c979cc8aa19e86bdfb28f0644e53feaf07643f5d19d126348  pool_0x1b932598da273069e1878e4a1505b6f833462210.json
3a5ca3d395ea558912b1caac15c006a1859e5ac525c970ee24f8a3d6e0acf620  pool_0x1b9a1120a17617d8ec4dc80b921a9a1c50caef7d.json
8df5e8d41c144e39514d7524a5b91563daa47753b136817f094e0434ae28ee01  pool_0x1f9b1a3ddedbf47b96c65f29c0586b678de2623b.json
12ef4130712b37c8332380f3b891953cc1fc6c6cad378547cea6c5f433002086  pool_0x246505db95e5a60d8524d52b9ed3dbaf6ee2584f.json
2d3c05226eb99b55faa3171b61a490881d54f4dc9a8796f630791cc433a8aa6d  pool_0x28b23d6ad0e440a37994eb49dc0b62dbcdea99a9.json
51c5cb2a392768b0976727214357caca113715c05fa9130317ba3053803feadf  pool_0x2f6c6e00e517944ee5efe310cd0b98a3fc61cb98.json
7c38e1b46bb3af1b15b954c21b792294ff4f32168f103761a43ba0fa476648e6  pool_0x3a8b7e833793ab906403238853db34bd3231508d.json
b3d906d59aee98396511ce0f5551b26b50667dd89dc36e4004641649ffcf7b6d  pool_0x44aa475ed44dda30f4fb81a0eec7c44aed01c7c0.json
487c61f415773ba7928c21d88a2b2585f1fea28020dd1fce3c959aa5d460c098  pool_0x489bf777382306bc7de514fa0f1a4c725aebe98f.json
3409dd6f5c36b9e39e24b76a409f28079556db9a22ce65f7f9f8e2140e6ececd  pool_0x5135c813d5f9d3624221b3bdbac805339fa5a591.json
cdfa48e878603b2764697ee62528a81744161cf490902cb2ab45c431b1bd6584  pool_0x51bd5e6d3da9064d59bcaa5a76776560ab42ceb8.json
9725b87e6d8c9efe5b73d852dee388076b3da1de06ffa231a4169a9121f15936  pool_0x52ddb7bd7ca73a20b6334cc08fcaca9906992a15.json
73f046190e4cb5cdf78d3c181075b3895320b7642f4f22a0f96830152da62e2d  pool_0x58b301e61705357cc78f815e604f9f9e486bc409.json
beefa9e724164949e52e643b3b6040bfe2a10ec625e8e36c68e8a65f6b0ce204  pool_0x5b0baf66718caabda49a4af32eb455c3b99b5821.json
84763b84334e1fda05c683f65f29fa7b9dd8d6477e60576b21de66ecf65f9582  pool_0x6b67112aa7b45e8cdc0a93b8d66a6a36e68ae8e5.json
38223e84c90b3d35be1694ad5cd7e26d8c786435e9643b5031418764ab56a7ab  pool_0x6eb8a743723216a0de043c554c85e447b0d59e2e.json
766bacc4c19bc9a201b964618d6220d429a589f2faa810fe738bf87171b1ff19  pool_0x752328a1e16d38933789860b8c09f6f2cc6c63d6.json
882f63c28c6796b7871ebb8a8b36e557fd2e941d54dc696e41cf10c3e83e831e  pool_0x755a52d29b24d6871899a84f476339183e9dc95d.json
599307bf1d77744e176fb0d84616b2f222162f459a3120ec4da0fea85d423f03  pool_0x775cac1c1b47250121291934d15912efb69213e9.json
9614e03560cafff2e0e157199db7381b40fe95c45e9539d2e10ddd8398c87ebe  pool_0x7c0b5d39765b221810d477e8f02d47a9badf018a.json
b89a191da776cd44e7f2ddf8aadd7dff75dbdd41fb472390d7e3d711699dd3ac  pool_0x8731bb0202deb31a93b3178b674e51c3cc49b285.json
e61db6fa1442c9092710847d913ba43631eb6231fc4e23586f3d71ed2158cc62  pool_0x8ab8fd902039a48e51735452b1f97c93cb034e80.json
e48f6bdfef0ade7f148173a20dd109462e95272f96fe3b619f96f489cda99887  pool_0x8e3ae967604dc4b4ba676e6d34ac603fb07fc921.json
60e2a77380af02f1450903b9247b7dc541e82a666e0d5c87803310d7be199bf4  pool_0x987c794c0786ee5cd6b34b7e32aa21098cd6b806.json
b993584e2207dc8f488c47fb33c7fa3201fc6cde59ca0f775da2abbba37ae117  pool_0x98a0004b8e9fe161369528a2e07de56c15a27d76.json
41ff7074ed24746007980d0962e37617e5cad3b88abc1df702cc27dc7a0c27ed  pool_0x9c487a70e3c2a81c5f1fb59251ab246ba857cbaf.json
ff1d0270a8be569d5c89bb6f350ebcb2e0fb2bbcda477313d61ecce4224f39cc  pool_0x9d49003383ab126c563ae2b7d545db003e657f53.json
2e345c2df1f2f6d11267b8ca8f97becec345eaecbad3b7ac3e007ad6de2c54b1  pool_0xa07bbf09b48e8d219774ac9b92622f5260a9c9f4.json
fe228790f06327001544fe21f5b37a2f3d3a197dc33b77b0c0b9ea4bc5e06e8d  pool_0xa60a504d92a1c95bda729c3f745b361ca822d6dd.json
1f203fd1c0c53e05d79b05de2fa95fb36eee374f92718b1694e259ae34bd57a8  pool_0xab6f06a33f38cba5a5312de24151cb91da2b0eb0.json
0d055ad511ac72db9c07148695015999cd496ad588e12ce5e23db04186121500  pool_0xb0d6a73b6c9014f58546dcb376f527f3aa71c4d4.json
53ab23271c223fcb8f2798c19b2eaaeda99d67ae9c722cc9008e8ae4a105ac10  pool_0xb3f3312252cade3a15eba318f6caaacb5e8097f4.json
9f6aff39b79880cf39691b4145ec6d0d50c8b5efc6c665163480b3db4c78865e  pool_0xbf121d987f9635ed6d2f7bb957fbbe163bdea0e0.json
82c2e6f424db64eea14a8762527c28cf3d6a23f46ca1d92800509da3cbf47b8f  pool_0xbf5e6e3c88c88cf6b17327f57a61a3a997f13a3b.json
b0a081208cd8dded711309db3f2a34d030065646470e59ee0b55ddf2c1948007  pool_0xc1716bea36140fa08476742d80cb28cecd492a6f.json
9d56a1cc6fca861913b131b96d6becc79c91424d3f6431e7db419b945758d07e  pool_0xc57061da1894ae58fb834f6db33e9a45cc4e7807.json
e4dd55e82a3b929d29649fd743146d821455ed7d684393a96bb1a8fd8dbf11dd  pool_0xc7db746edd16bbde9a75aa6114400709041da561.json
3c1536090bb5c81dd6c007ea05b41f7476cac10295ef1a11aeaed856219545ad  pool_0xc9df4ae113fa7cb67ce1779cb1560d1f43c2d373.json
2fdfcb74a4c437173c6a6c5c3f36b199ac689ad6e7b2af5cb1a1005879cff5ff  pool_0xd33f3c50df98d3bf0f47d8848340874a3bf9362d.json
7d9b3a2990db80469642b74abe0eb8e67c651ed84b808d53cb5d91a6dc150f96  pool_0xd405b976ac01023c9064024880999fc450a8668b.json
c7e5446d98ca8e198ad5a68b95e189197e3cf8f6901cbf4cc0c21dc484b64e6e  pool_0xd688108301df5d939bf272eb378429488a72ea52.json
95972b60efceb5ff91e9a15560c7ec3b00520256c5340d06c748647b1cace3c6  pool_0xd6f0ba7ec72ff3974b02c18ed1fc33da77434d41.json
ecc0b6a3c843000df9867212d58744ea78926d46b55f8a024113ded531c0a7c1  pool_0xde045462392a86a313239c5cbac87f7f57a92402.json
420e0007cd749d67edc484954f05a429327f459c73ecd8b413fe6e9635514b11  pool_0xf2a4e4261fcdfbb891bcf703640fbe713c6cd0fe.json
2d41b37f2f835441edca3e9bdfabe92d3eed23a422300d9528081c9b8390ca24  pool_0xf6814792050ccd1c603d8a95468d6d9587ecbdaf.json
13cc075b601b21faa7ebff5b6c403e6ef54f920a116ce3dd14eb147381ccc34d  pool_0xf860e573fd87669e9004c108cd8245962f96b444.json
a8bc9b144d6533d59e05eecff84e22dcdafbd73fe6fed7f61668de640d8ad8b5  pool_0xff4840a06708e53b620e2670295265029fa72a78.json
022acab43483c18114f3e4cabdb4f61e0855b4b73d1400406e20c49a0821f534  sourcify.json
