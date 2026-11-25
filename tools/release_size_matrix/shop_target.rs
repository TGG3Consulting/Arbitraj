#[tokio::test]
async fn shop_target_30327() {
    for (block,word) in [(133_910_892u64,-491i16),(133_910_895,-489i16)] {
        for width in [6i16,12] {
            let parts=shopinx::scene_at(&format!("tests/data/shopinx-{block}"),block,word,width).await;
            configure(&parts,"graph").await;
            shopinx::partial(&parts,Some(true)).await;
            for qty in [num("45342.031709"),num("45886.738452")] {
                let q=shopinx::full(&parts.quoting(),Side::Buy,qty).await.unwrap();
                assert_eq!(q.fillable_qty,qty);
                println!("SHOP_30327 {}",json!({"block":block,"width":width,"qty":qty,"fillable":q.fillable_qty,"avg":q.avg_price,"debit_usdt":q.avg_price*q.fillable_qty,"route":q.route,"costs":q.costs}));
            }
        }
    }
}
