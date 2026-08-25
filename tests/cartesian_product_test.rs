use sql_optimizer_cli::core::types::SchemaSnapshot;
use sql_optimizer_cli::patterns::cartesian_product::detect_cartesian_product;

#[test]
fn detects_cross_join() {
    let query = "SELECT * FROM users CROSS JOIN orders";
    let schema = SchemaSnapshot::default();
    let recs = detect_cartesian_product(query, &schema);
    assert!(!recs.is_empty(), "should detect CROSS JOIN");
    assert!(recs.iter().any(|r| r.description.contains("CROSS JOIN")));
}

#[test]
fn detects_implicit_cartesian() {
    // Multiple tables in FROM with no JOIN — implicit cartesian
    let query = "SELECT * FROM users, orders";
    let schema = SchemaSnapshot::default();
    let recs = detect_cartesian_product(query, &schema);
    assert!(!recs.is_empty(), "should detect implicit cartesian product");
}

#[test]
fn no_false_positive_on_inner_join() {
    let query = "SELECT * FROM users JOIN orders ON users.id = orders.user_id";
    let schema = SchemaSnapshot::default();
    let recs = detect_cartesian_product(query, &schema);
    assert!(recs.is_empty(), "INNER JOIN with ON should not trigger");
}

#[test]
fn no_false_positive_on_single_table() {
    let query = "SELECT * FROM users WHERE id = 1";
    let schema = SchemaSnapshot::default();
    let recs = detect_cartesian_product(query, &schema);
    assert!(recs.is_empty(), "single table query should not trigger");
}
