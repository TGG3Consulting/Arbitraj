from pathlib import Path
import sys
p = Path(sys.argv[1]) / 'tests/size_matrix_routes.inc'
s = p.read_text()
anchor = 'let mut p=native_params(&s,guess,floor,max_evals);'
assert s.count(anchor) == 1
s = s.replace(anchor, anchor + '''
                if std::env::var("SIZING_ARM").as_deref()==Ok("on") && matches!(s.buy,Market::Router(_)) && matches!(s.sell,Market::Ladder(_)) {
                    p.ext.plateau=arb::core::size_plateau::Arrival::new(s.withdraw, (Decimal::ONE-s.arrive_keep)*Decimal::ONE_HUNDRED);
                }
''')
p.write_text(s)
