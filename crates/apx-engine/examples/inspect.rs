fn main() {
    let source = std::env::args().nth(1).unwrap_or(
        "class Probe { public Integer count { get { return 3; } private set { value++; } } }"
            .into(),
    );
    let n = apx_engine::syntax::parse(&source, "inspect.cls").unwrap();
    fn show(n: &apx_engine::syntax::Node, depth: usize) {
        println!(
            "{}{} {}",
            " ".repeat(depth),
            n.kind,
            n.text().chars().take(80).collect::<String>()
        );
        for (field, child) in &n.children {
            println!("{}field {:?}", " ".repeat(depth + 1), field);
            show(child, depth + 2);
        }
    }
    show(&n, 0);
}
