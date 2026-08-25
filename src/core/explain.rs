use crate::core::types::QueryPlan;

pub fn plain_explain_summary(plan: &Option<QueryPlan>) -> Option<String> {
    let plan = plan.as_ref()?;
    let root = plan.root.as_ref()?;

    // Build a concise summary from available information
    let mut parts = Vec::new();

    // The node_type from SQLite already contains a descriptive string like
    // "sequential scan on 'users' (no index)" or "index search on 'users' using index 'idx_email'"
    // For Postgres/MySQL, node_type is just the node type name (e.g. "Seq Scan")
    parts.push(root.node_type.clone());

    if let Some(rows) = root.rows {
        parts.push(format!("~{} rows", rows as i64));
    }
    if let Some(cost) = root.cost {
        parts.push(format!("cost {:.1}", cost));
    }

    // For Postgres/MySQL plans where index_used is separate from node_type
    if plan.engine != "sqlite" {
        if let Some(idx) = &root.index_used {
            parts.push(format!("index: {}", idx));
        }
    }

    Some(parts.join(", "))
}
