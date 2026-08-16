use polars::polars_utils::itertools::Itertools;
use polars::prelude::*;
use crate::pipeline::model::config::{PipelineActiveEventAction, PipelineActiveEventPredicate, PipelineActiveEventRule};

/// Return Polars LazyFrame representing evaluation of given PipelineActiveEventRules
pub fn evaluate_active_window_rules(df: LazyFrame, rules: &Vec<PipelineActiveEventRule>) -> LazyFrame {
    rules.into_iter()
        .map(|rule| {
            rule.actions.iter().map(|action| {
                active_window_action(active_window_predicate(&rule.predicate), action)
            }).collect_vec()
        })
        .fold(df, |df, exprs| {
            df.with_columns(exprs)
        })
}

/// Return Polars expression for `PipelineActiveEventPredicate`
pub fn active_window_predicate(p: &PipelineActiveEventPredicate) -> Expr {
    match p {
        PipelineActiveEventPredicate::AttributeValue { name, value } => {
            col(name.as_str()).eq(lit(value.as_str())).fill_null(lit(false))
        }
        PipelineActiveEventPredicate::AttributeRegex { name, regex } => {
            col(name.as_str()).str().contains(lit(regex.as_str()), false)
        }
        PipelineActiveEventPredicate::HasTag(tag) => {
            col("tags").list().contains(lit(tag.as_str()), false)
        }
        PipelineActiveEventPredicate::IsMobile => {
            col("isMobile").eq(lit(true))
        }
        PipelineActiveEventPredicate::IdleForGreaterThanSec(value) => {
            col("idleFor").gt(lit(value * 1000).cast(DataType::Duration(TimeUnit::Milliseconds)))
        }
        PipelineActiveEventPredicate::And(qs) => {
            qs.into_iter()
                .map(|q| active_window_predicate(q))
                .fold(lit(true), |acc, expr| acc.and(expr))
        }
        PipelineActiveEventPredicate::Or(qs) => {
            qs.into_iter()
                .map(|q| active_window_predicate(q))
                .fold(lit(false), |acc, expr| acc.or(expr))
        }
        PipelineActiveEventPredicate::Not(q) => {
            active_window_predicate(q).not()
        }
    }
}

/// Return Polars expression for `PipelineActiveEventAction`
pub fn active_window_action(predicate: Expr, action: &PipelineActiveEventAction) -> Expr {
    match action {
        PipelineActiveEventAction::AddTag(tag) => {
            when(predicate.and(col("tags").list().contains(lit(tag.as_str()), false).not()))
                .then(concat_list([
                    col("tags"),
                    lit(Series::new("".into(), vec![tag.clone()])),
                ]).unwrap())
                .otherwise(col("tags"))
                .alias("tags")
        }
        PipelineActiveEventAction::SetAttribute { name, value } => {
            when(predicate)
                .then(lit(value.as_str()))
                .otherwise(col(name.as_str()))
                .alias(name.as_str())
        }
        PipelineActiveEventAction::Ignore => {
            when(predicate)
                .then(lit(true))
                .otherwise(col("ignore"))
                .alias("ignore")
        }
    }
}
