"""Публичные схемы Treble хранят точный блок создания и адрес custom deployer каждого рынка."""


QUERY = """
query TrebleMarkets($block: Bytes!, $after: ID!) {
  _meta(block:{hash:$block}) { block { hash } hasIndexingErrors }
  pools(first:1000, orderBy:id, orderDirection:asc, where:{id_gt:$after}, block:{hash:$block}) {
    id createdAtBlockNumber deployer token0 { id } token1 { id }
  }
}
"""


def placements(session, graph, key, canonical):
    url = "https://gateway.thegraph.com/api/subgraphs/id/" + graph
    cursor = ""
    result = {}
    while True:
        reply = session.post(url, headers={"Authorization": "Bearer " + key},
                             json={"query": QUERY, "variables": {"block": canonical, "after": cursor}},
                             timeout=22)
        reply.raise_for_status()
        body = reply.json()
        if body.get("errors") or not isinstance(body.get("data"), dict):
            raise ValueError("Официальный подграф не подтвердил запрошенный исторический блок")
        meta = body["data"]["_meta"]
        if meta["hasIndexingErrors"] or meta["block"]["hash"].lower() != canonical:
            raise ValueError("Подграф и канонический блок Treble расходятся")
        pools = body["data"]["pools"]
        for pool in pools:
            addr = pool["id"].lower()
            if addr <= cursor or addr in result:
                raise ValueError("Неполная страница подграфа Treble")
            cursor = addr
            result[addr] = (int(pool["createdAtBlockNumber"]),pool["deployer"].lower(),
                            [pool[t]["id"].lower() for t in ("token0","token1")])
        if len(pools) < 1000:
            return result

