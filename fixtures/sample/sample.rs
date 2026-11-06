// Fixture: exported API stays quiet, private unused code is reported.

fn used_helper(input: &str) -> String {
    input.trim().to_lowercase()
}

fn ghost_one() -> usize {
    ghost_two() + 1
}

fn ghost_two() -> usize {
    41
}

pub fn public_entry(input: &str) -> String {
    used_helper(input)
}

fn main() {
    let text = "  hello  ";
    println!("{}", used_helper(text));
    // a comment mentioning ghost_one must not count as a use
    let s = "a string mentioning ghost_two either";
    println!("{}", s.len());
}

// The harness calls this itself: no call site exists, so bench_only must not
// be reported dead just because its only caller looks unreachable.
#[bench]
fn bench_the_helper(b: &mut test::Bencher) {
    b.iter(|| bench_only(3));
}

fn bench_only(x: usize) -> usize {
    x * 7
}
