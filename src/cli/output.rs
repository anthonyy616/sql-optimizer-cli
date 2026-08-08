use anyhow::Result;
use colored::*;
use std::fmt::Write as _;

use crate::core::types::*;

pub struct OutputFormatter {
    format: OutputFormat,
}

impl OutputFormatter {
    pub fn new(format: OutputFormat) -> Self {
        Self { format }
    }

    pub fn format(&self, result: &AnalysisResult) -> Result<()> {
        match self.format {
            OutputFormat::Text => self.format_text(result),
            OutputFormat::Json | OutputFormat::Yaml | OutputFormat::Markdown => {
                println!("{}", self.render(result)?);
                Ok(())
            }
        }
    }

    pub fn render(&self, result: &AnalysisResult) -> Result<String> {
        match self.format {
            OutputFormat::Text => Err(anyhow::anyhow!(
                "text output is intended for stdout formatting only"
            )),
            OutputFormat::Json => self.render_json(result),
            OutputFormat::Yaml => self.render_yaml(result),
            OutputFormat::Markdown => self.render_markdown(result),
        }
    }

    fn format_text(&self, result: &AnalysisResult) -> Result<()> {
        println!("{}", "SQL Analysis Results".bold().cyan());
        println!("{}", "===================".bold().cyan());
        println!(
            "Query: {}",
            &result.query[..std::cmp::min(result.query.len(), 60)]
        );
        if result.query.len() > 60 {
            println!("...");
        }
        println!("Database: {:?}", result.database_type);
        println!("Analysis Time: {}ms", result.execution_time_ms);
        println!();

        // Recommendations
        if !result.recommendations.is_empty() {
            println!("{}", "OPTIMIZATION OPPORTUNITIES:".bold().yellow());
            for (i, rec) in result.recommendations.iter().enumerate() {
                println!("{}. {}", i + 1, rec.description.bright_yellow());
                if let Some(suggestion) = &rec.sql_suggestion {
                    println!("   Suggestion: {}", suggestion.dimmed());
                }
                println!(
                    "   Estimated improvement: {:.1}%",
                    rec.estimated_improvement * 100.0
                );
                println!();
            }
        } else {
            println!("{}", "No optimization opportunities found.".green());
            println!();
        }

        // Security analysis
        println!("{}", "SECURITY ANALYSIS:".bold().magenta());
        if result.security_issues.is_empty() {
            println!("{}", "✓ No security issues detected".green());
        } else {
            for (i, issue) in result.security_issues.iter().enumerate() {
                let severity_color = match issue.severity {
                    Severity::Low => colored::Color::Blue,
                    Severity::Medium => colored::Color::Yellow,
                    Severity::High => colored::Color::Red,
                    Severity::Critical => colored::Color::Red,
                };
                println!("{}. {}", i + 1, issue.description.color(severity_color));
                println!("   Severity: {:?}", issue.severity);
            }
        }

        if let Some(schema) = &result.schema_snapshot {
            println!();
            println!("{}", "SCHEMA SNAPSHOT:".bold().blue());
            println!("Tables discovered: {}", schema.tables.len());
            for table in schema.tables.iter().take(5) {
                println!(
                    "- {} ({} columns, {} indexes)",
                    table.name,
                    table.columns.len(),
                    table.indexes.len()
                );
            }
        }

        if let Some(preview) = &result.row_preview {
            println!();
            println!("{}", "ROW PREVIEW:".bold().green());
            println!("Limit: {}", preview.limit);
            println!("Truncated: {}", preview.truncated);
            if !preview.columns.is_empty() {
                println!("Columns: {}", preview.columns.join(", "));
            }
            for row in preview.rows.iter().take(5) {
                println!("- {}", row.join(" | "));
            }
        }

        if let Some(plan) = &result.explain_plan {
            println!();
            println!("{}", "EXPLAIN PLAN:".bold().cyan());
            println!("Engine: {}", plan.engine);
            if let Some(root) = &plan.root {
                println!("Root node: {}", root.node_type);
                if let Some(rows) = root.rows {
                    println!("Estimated rows: {}", rows);
                }
                if let Some(cost) = root.cost {
                    println!("Estimated cost: {}", cost);
                }
                if let Some(index) = &root.index_used {
                    println!("Index used: {}", index);
                }
            }
            // Plain-English summary
            if let Some(summary) = crate::core::explain::plain_explain_summary(&result.explain_plan)
            {
                println!("\nEXPLAIN SUMMARY: {}", summary);
            }
        }

        Ok(())
    }

    fn render_json(&self, result: &AnalysisResult) -> Result<String> {
        Ok(serde_json::to_string_pretty(result)?)
    }

    fn render_yaml(&self, result: &AnalysisResult) -> Result<String> {
        Ok(serde_yaml::to_string(result)?)
    }

    fn render_markdown(&self, result: &AnalysisResult) -> Result<String> {
        let mut output = String::new();

        writeln!(&mut output, "# SQL Analysis Results")?;
        writeln!(&mut output)?;
        writeln!(&mut output, "- **Query:** `{}`", escape_markdown_inline(&result.query))?;
        writeln!(&mut output, "- **Database:** `{:?}`", result.database_type)?;
        writeln!(&mut output, "- **Analysis Time:** {}ms", result.execution_time_ms)?;
        writeln!(&mut output)?;

        writeln!(&mut output, "## Optimization Opportunities")?;
        if result.recommendations.is_empty() {
            writeln!(&mut output, "No optimization opportunities found.")?;
        } else {
            for (i, rec) in result.recommendations.iter().enumerate() {
                writeln!(&mut output, "{}. {}", i + 1, rec.description)?;
                if let Some(suggestion) = &rec.sql_suggestion {
                    writeln!(
                        &mut output,
                        "   - Suggestion: `{}`",
                        escape_markdown_inline(suggestion)
                    )?;
                }
                writeln!(
                    &mut output,
                    "   - Estimated improvement: {:.1}%",
                    rec.estimated_improvement * 100.0
                )?;
            }
        }
        writeln!(&mut output)?;

        writeln!(&mut output, "## Security Analysis")?;
        if result.security_issues.is_empty() {
            writeln!(&mut output, "- No security issues detected")?;
        } else {
            for issue in &result.security_issues {
                writeln!(
                    &mut output,
                    "- {} (Severity: `{:?}`)",
                    issue.description,
                    issue.severity
                )?;
            }
        }

        if let Some(schema) = &result.schema_snapshot {
            writeln!(&mut output)?;
            writeln!(&mut output, "## Schema Snapshot")?;
            writeln!(&mut output, "- Tables discovered: {}", schema.tables.len())?;
            for table in schema.tables.iter().take(5) {
                writeln!(
                    &mut output,
                    "  - {} ({} columns, {} indexes)",
                    table.name,
                    table.columns.len(),
                    table.indexes.len()
                )?;
            }
        }

        if let Some(preview) = &result.row_preview {
            writeln!(&mut output)?;
            writeln!(&mut output, "## Row Preview")?;
            writeln!(&mut output, "- Limit: {}", preview.limit)?;
            writeln!(&mut output, "- Truncated: {}", preview.truncated)?;
            if !preview.columns.is_empty() {
                writeln!(&mut output, "- Columns: {}", preview.columns.join(", "))?;
            }
            for row in preview.rows.iter().take(5) {
                writeln!(&mut output, "  - {}", row.join(" | "))?;
            }
        }

        if let Some(plan) = &result.explain_plan {
            writeln!(&mut output)?;
            writeln!(&mut output, "## Explain Plan")?;
            writeln!(&mut output, "- Engine: `{}`", plan.engine)?;
            if let Some(root) = &plan.root {
                writeln!(&mut output, "- Root node: `{}`", root.node_type)?;
                if let Some(rows) = root.rows {
                    writeln!(&mut output, "- Estimated rows: {}", rows)?;
                }
                if let Some(cost) = root.cost {
                    writeln!(&mut output, "- Estimated cost: {}", cost)?;
                }
                if let Some(index) = &root.index_used {
                    writeln!(
                        &mut output,
                        "- Index used: `{}`",
                        escape_markdown_inline(index)
                    )?;
                }
            }
            if let Some(summary) = crate::core::explain::plain_explain_summary(&result.explain_plan)
            {
                writeln!(&mut output)?;
                writeln!(&mut output, "**Explain Summary:** {}", summary)?;
            }
        }

        Ok(output)
    }
}

fn escape_markdown_inline(input: &str) -> String {
    input.replace('`', "\\`")
}
