kind = 'deal' AND CASE WHEN json_valid(body) THEN (
    (json_extract(body,'$.plan.settlement_cycle') IS NOT NULL
        AND coalesce(json_extract(body,'$.step'),'') NOT IN ('Closed','Halted','TakenOver'))
    OR (json_extract(body,'$.step') IN ('Buying','Halted')
        AND json_extract(body,'$.buy') IS NULL AND json_extract(body,'$.plan.size.budget') IS NOT NULL)
    OR json_extract(body,'$.group.wait') IS NOT NULL
    OR coalesce(json_extract(body,'$.usdt_return.state'),'') IN ('pending','not_returned','retrying')
    OR instr(json_extract(body,'$.usdt_return_parts'),'"pending"') > 0
    OR instr(json_extract(body,'$.usdt_return_parts'),'"not_returned"') > 0
    OR instr(json_extract(body,'$.usdt_return_parts'),'"retrying"') > 0
) ELSE 1 END
