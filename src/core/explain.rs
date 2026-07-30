use crate::core::types::QueryPlan;

pub fn plain_explain_summary(plan: &Option<QueryPlan>) -> Option<String> {
    let plan = plan.as_ref()?;
    let root = plan.root.as_ref()?;

    // Simple heuristic summary based on node type, rows, and index usage
    let mut parts = Vec::new();
    parts.push(root.node_type.clone());
    if let Some(rows) = root.rows {
        parts.push(format!("~{} rows", rows as i64));
    }
    if let Some(cost) = root.cost {
        parts.push(format!("cost {:.1}", cost));
    }
    if let Some(idx) = &root.index_used {
        parts.push(format!("index: {}", idx));
    } else {
        parts.push("no index used".to_string());
    }

    Some(parts.join(", "))
}
