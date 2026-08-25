use sql_optimizer_cli::core::fingerprint::{canonicalize_query, fingerprint};

#[test]
fn canonicalize_strips_literals_and_normalizes() {
    let q1 = "SELECT id FROM users WHERE id = 1 AND name = 'Alice'";
    let q2 = "select id from users where id = 2 and name = 'Bob'";

    let c1 = canonicalize_query(q1);
    let c2 = canonicalize_query(q2);

    assert_eq!(c1, c2);
    assert_eq!(fingerprint(q1), fingerprint(q2));
}
