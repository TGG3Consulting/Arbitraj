# Ekubo (Arbitrum One) — дополнительная запись (решение 454)

tools/arbitrum_record/ekubo_more.py: только чтение. Блок B = 526041266 (0x1f5ac0b2), время 1795207849.
PoolInitialized с блока 416469411 (квитанция развёртывания Core в contracts/evmtest/vendor/ekubo/broadcast/DeployAll.s.sol/42161/run-1767194711580.json) до B: {"total": 22, "native": 8, "extensions": {"0x0000000000000000000000000000000000000000": 12, "0xd4f1060cb9c1a13e1d2d20379b8aa2cf7541ed9b": 1, "0x406a6e4f256dd43bd8835ce84d108e1e4476d5e4": 1, "0x409bd88ab297f351d20f576893f989d0b8f85049": 1, "0xd47f1b1edcfeabb08f6ebd8fc337c27e636c75ba": 2, "0x611953b8b2ecfe5692e189b3ce4e647c6e83455e": 2, "0x589dffa001c81b691b4e79a0048625f9b80b726a": 1, "0x659f0793f6c9d4eec32967df986c52e8b0a4629f": 1, "0x6ae4d63bcd054c5698cac06a7bc5754632ee267e": 1}, "not_in_key_index": 22}.
Решение 460: поиск PoolInitialized — до чтения состояния; блок B взят после поиска (узел хранит только недавнее состояние), хвост логов до B дочитан.

## SHA-256

5731232e9a6454c96143969d8614ee309abb76c0de7c8c76cded79e04f09f3b1  more_meta.json
116474431b1d4ce865880dc229de5cbeab4993cc7fddaa381f6945fb3c70c3b0  pool_initialized.json
21a2676f158c98a84fed5e2b464943e7eb96b7220bb834612dc7bd528b565911  native_pools.json
08918ba8017faae1fc0c0561c5d4a87d223fc4ef2c87f2e4fd6f577dd297d39d  twamm_pools.json
3f5094782ba0a6aa3ed702df7e212087d539ee13f69c365ca90e22a199e183e0  core_load.json
