use anyhow::Context;
use sea_query::{
    Alias, CommonTableExpression, Expr, Func, JoinType, Order, PostgresQueryBuilder, Query,
    SelectStatement, WithClause,
};
use sea_query_binder::{SqlxBinder, SqlxValues};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, FromInto};
use std::collections::HashMap;

use crate::{
    collection::Collection,
    debug_sea_query,
    filter_builder::FilterBuilder,
    model::ModelRuntime,
    models,
    pipeline::Pipeline,
    remote_embeddings::build_remote_embeddings,
    types::{CustomU64Convertor, IntoTableNameAndSchema, Json, SIden},
};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
struct ValidField {
    query: String,
    parameters: Option<Json>,
    full_text_filter: Option<String>,
    boost: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
struct ValidQueryActions {
    fields: Option<HashMap<String, ValidField>>,
    filter: Option<Json>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
struct ValidDocument {
    keys: Option<Vec<String>>,
}

const fn default_limit() -> u64 {
    10
}

#[serde_as]
#[derive(Debug, Deserialize, Serialize, Clone)]
// #[serde(deny_unknown_fields)]
pub struct ValidQuery {
    query: ValidQueryActions,
    // Need this when coming from JavaScript as everything is an f64 from JS
    #[serde(default = "default_limit")]
    #[serde_as(as = "FromInto<CustomU64Convertor>")]
    limit: u64,
    // Document related items
    document: Option<ValidDocument>,
}

pub async fn build_sqlx_query(
    query: Json,
    collection: &Collection,
    pipeline: &Pipeline,
    include_pipeline_table_cte: bool,
    prefix: Option<&str>,
) -> anyhow::Result<(SelectStatement, Vec<CommonTableExpression>)> {
    let valid_query: ValidQuery = serde_json::from_value(query.0)?;
    let limit = valid_query.limit;
    let fields = valid_query.query.fields.unwrap_or_default();

    let prefix = prefix.unwrap_or("");

    if fields.is_empty() {
        anyhow::bail!("at least one field is required to search over")
    }

    let pipeline_table = format!("{}.pipelines", collection.name);
    let documents_table = format!("{}.documents", collection.name);

    let mut queries = Vec::new();
    let mut ctes = Vec::new();

    if include_pipeline_table_cte {
        let mut pipeline_cte = Query::select();
        pipeline_cte
            .from(pipeline_table.to_table_tuple())
            .columns([models::PipelineIden::Schema])
            .and_where(Expr::col(models::PipelineIden::Name).eq(&pipeline.name));
        let mut pipeline_cte = CommonTableExpression::from_select(pipeline_cte);
        pipeline_cte.table_name(Alias::new("pipeline"));
        ctes.push(pipeline_cte);
    }

    for (key, vf) in fields {
        let model_runtime = pipeline
            .parsed_schema
            .as_ref()
            .map(|s| {
                // Any of these errors means they have a malformed query
                anyhow::Ok(
                    s.get(&key)
                        .as_ref()
                        .context(format!("Bad query - {key} does not exist in schema"))?
                        .semantic_search
                        .as_ref()
                        .context(format!(
                            "Bad query - {key} does not have any directive to semantic_search"
                        ))?
                        .model
                        .runtime,
                )
            })
            .transpose()?
            .unwrap_or(ModelRuntime::Python);

        let chunks_table = format!("{}_{}.{}_chunks", collection.name, pipeline.name, key);
        let embeddings_table = format!("{}_{}.{}_embeddings", collection.name, pipeline.name, key);

        let mut query = Query::select();

        let boost = vf.boost.unwrap_or(1.);

        match model_runtime {
            ModelRuntime::Python => {
                // Build the embedding CTE
                let mut embedding_cte = Query::select();
                embedding_cte.expr_as(
                    Func::cust(SIden::Str("pgml.embed")).args([
                        Expr::cust(format!(
                            "transformer => (SELECT schema #>> '{{{key},semantic_search,model}}' FROM pipeline)",
                        )),
                        Expr::cust_with_values("text => $1", [vf.query]),
                        Expr::cust_with_values("kwargs => $1", [vf.parameters.unwrap_or_default().0]),
                    ]),
                    Alias::new("embedding"),
                );
                let mut embedding_cte = CommonTableExpression::from_select(embedding_cte);
                embedding_cte.table_name(Alias::new(format!("{prefix}{key}_embedding")));
                ctes.push(embedding_cte);

                query
                    .expr(Expr::cust(format!(
                        r#"(1 - (embeddings.embedding <=> (SELECT embedding FROM "{prefix}{key}_embedding")::vector)) * {boost} AS score"#
                    )))
                    .order_by_expr(Expr::cust(format!(
                        r#"embeddings.embedding <=> (SELECT embedding FROM "{prefix}{key}_embedding")::vector"#
                    )), Order::Asc);
            }
            ModelRuntime::OpenAI => {
                // We can unwrap here as we know this is all set from above
                let model = &pipeline
                    .parsed_schema
                    .as_ref()
                    .unwrap()
                    .get(&key)
                    .unwrap()
                    .semantic_search
                    .as_ref()
                    .unwrap()
                    .model;

                // Get the remote embedding
                let embedding = {
                    let remote_embeddings = build_remote_embeddings(
                        model.runtime,
                        &model.name,
                        vf.parameters.as_ref(),
                    )?;
                    let mut embeddings =
                        remote_embeddings.embed(vec![vf.query.to_string()]).await?;
                    std::mem::take(&mut embeddings[0])
                };

                // Build the score CTE
                query
                    .expr(Expr::cust_with_values(
                        format!(
                            r#"(1 - (embeddings.embedding <=> $1::vector)) * {boost} AS score"#
                        ),
                        [embedding.clone()],
                    ))
                    .order_by_expr(
                        Expr::cust_with_values(
                            r#"embeddings.embedding <=> $1::vector"#,
                            [embedding],
                        ),
                        Order::Asc,
                    );
            }
        }

        query
            .column((SIden::Str("documents"), SIden::Str("id")))
            .column((SIden::Str("chunks"), SIden::Str("chunk")))
            .column((SIden::Str("documents"), SIden::Str("document")))
            .from_as(embeddings_table.to_table_tuple(), Alias::new("embeddings"))
            .join_as(
                JoinType::InnerJoin,
                chunks_table.to_table_tuple(),
                Alias::new("chunks"),
                Expr::col((SIden::Str("chunks"), SIden::Str("id")))
                    .equals((SIden::Str("embeddings"), SIden::Str("chunk_id"))),
            )
            .join_as(
                JoinType::InnerJoin,
                documents_table.to_table_tuple(),
                Alias::new("documents"),
                Expr::col((SIden::Str("documents"), SIden::Str("id")))
                    .equals((SIden::Str("chunks"), SIden::Str("document_id"))),
            )
            .limit(limit);

        if let Some(filter) = &valid_query.query.filter {
            let filter = FilterBuilder::new(filter.clone().0, "documents", "document").build()?;
            query.cond_where(filter);
        }

        if let Some(full_text_search) = &vf.full_text_filter {
            let full_text_table =
                format!("{}_{}.{}_tsvectors", collection.name, pipeline.name, key);
            query
                .and_where(Expr::cust_with_values(
                format!(
                    r#"tsvectors.ts @@ plainto_tsquery((SELECT oid FROM pg_ts_config WHERE cfgname = (SELECT schema #>> '{{{key},full_text_search,configuration}}' FROM pipeline)), $1)"#,
                ),
                    [full_text_search],
                ))
                .join_as(
                JoinType::InnerJoin,
                full_text_table.to_table_tuple(),
                Alias::new("tsvectors"),
                Expr::col((SIden::Str("tsvectors"), SIden::Str("chunk_id")))
                    .equals((SIden::Str("embeddings"), SIden::Str("chunk_id")))
            );
        }

        let mut wrapper_query = Query::select();

        // Allows filtering on which keys to return with the document
        if let Some(document) = &valid_query.document {
            if let Some(keys) = &document.keys {
                let document_queries = keys
                    .iter()
                    .map(|key| format!("'{key}', document #> '{{{key}}}'"))
                    .collect::<Vec<String>>()
                    .join(",");
                wrapper_query.expr_as(
                    Expr::cust(format!("jsonb_build_object({document_queries})")),
                    Alias::new("document"),
                );
            } else {
                wrapper_query.column(SIden::Str("document"));
            }
        } else {
            wrapper_query.column(SIden::Str("document"));
        }

        wrapper_query
            .columns([SIden::Str("chunk"), SIden::Str("score")])
            .from_subquery(query, Alias::new("s"));

        queries.push(wrapper_query);
    }

    // Union all of the queries together
    let mut query = queries.pop().context("no query")?;
    for q in queries.into_iter() {
        query.union(sea_query::UnionType::All, q);
    }

    // Resort and limit
    query
        .order_by(SIden::Str("score"), Order::Desc)
        .limit(limit);

    Ok((query, ctes))
}

pub async fn build_vector_search_query(
    query: Json,
    collection: &Collection,
    pipeline: &Pipeline,
) -> anyhow::Result<(String, SqlxValues)> {
    let (query, ctes) = build_sqlx_query(query, collection, pipeline, true, None).await?;
    let mut with_clause = WithClause::new();
    for cte in ctes {
        with_clause.cte(cte);
    }
    let (sql, values) = query.with(with_clause).build_sqlx(PostgresQueryBuilder);

    debug_sea_query!(VECTOR_SEARCH, sql, values);
    Ok((sql, values))
}
