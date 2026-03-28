use anyhow::Result;
use colored::*;

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
            OutputFormat::Json => self.format_json(result),
            OutputFormat::Yaml => self.format_yaml(result),
        }
    }

    fn format_text(&self, result: &AnalysisResult) -> Result<()> {
        println!("{}", "SQL Analysis Results".bold().cyan());
        println!("{}", "===================".bold().cyan());
        println!("Query: {}", &result.query[..std::cmp::min(result.query.len(), 60)]);
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
                println!("   Estimated improvement: {:.1}%", rec.estimated_improvement * 100.0);
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

        Ok(())
    }

    fn format_json(&self, result: &AnalysisResult) -> Result<()> {
        let json = serde_json::to_string_pretty(result)?;
        println!("{}", json);
        Ok(())
    }

    fn format_yaml(&self, result: &AnalysisResult) -> Result<()> {
        let yaml = serde_yaml::to_string(result)?;
        println!("{}", yaml);
        Ok(())
    }
}
