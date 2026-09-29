// Comments with characters which must be represented by multibyte.

// フー
use foo;
// バー
use bar;

impl MyStruct {
    // コメント
    fn f1() {} // こんにちは
    fn f2() {} // ありがとう
               // コメント
}

trait MyTrait {
    // コメント
    fn f1() {} // こんにちは
    fn f2() {} // ありがとう
               // コメント
}

fn main() {
    // コメント
    let x = 1; // Ｘ
    println!(
        "x = {}",
        x,        // Ｘ
    );
    // コメント
}
