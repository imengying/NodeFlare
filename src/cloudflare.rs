use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, Fetch, Method, Request, RequestInit, Result};

const GRAPHQL_URL: &str = "https://api.cloudflare.com/client/v4/graphql";
const USAGE_QUERY: &str = r#"
query CloudflareUsage(
  $accountTag: string!,
  $start: Date,
  $end: Date,
  $startTime: Time!,
  $endTime: Time!
) {
  viewer {
    accounts(filter: { accountTag: $accountTag }) {
      d1AnalyticsAdaptiveGroups(
        limit: 10000,
        filter: { date_geq: $start, date_leq: $end }
      ) {
        sum { rowsRead rowsWritten }
      }
      workersInvocationsAdaptive(
        limit: 10000,
        filter: { datetime_geq: $startTime, datetime_leq: $endTime }
      ) {
        sum { requests }
      }
    }
  }
}
"#;

#[derive(Debug, Serialize)]
pub struct UsagePeriod {
    pub rows_read: i64,
    pub rows_written: i64,
    pub workers_requests: i64,
}

#[derive(Debug, Serialize)]
pub struct CloudflareUsage {
    pub today: UsagePeriod,
    pub yesterday: UsagePeriod,
}

#[derive(Serialize)]
struct GraphQlRequest<'a> {
    query: &'a str,
    variables: GraphQlVariables<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlVariables<'a> {
    account_tag: &'a str,
    start: &'a str,
    end: &'a str,
    start_time: &'a str,
    end_time: &'a str,
}

#[derive(Deserialize)]
struct GraphQlResponse {
    data: Option<GraphQlData>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
struct GraphQlData {
    viewer: Viewer,
}

#[derive(Deserialize)]
struct Viewer {
    accounts: Vec<AccountUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsage {
    #[serde(default)]
    d1_analytics_adaptive_groups: Vec<D1Group>,
    #[serde(default)]
    workers_invocations_adaptive: Vec<WorkersGroup>,
}

#[derive(Deserialize)]
struct D1Group {
    sum: D1Sum,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct D1Sum {
    #[serde(default)]
    rows_read: i64,
    #[serde(default)]
    rows_written: i64,
}

#[derive(Deserialize)]
struct WorkersGroup {
    sum: WorkersSum,
}

#[derive(Deserialize)]
struct WorkersSum {
    #[serde(default)]
    requests: i64,
}

pub fn date_from_unix_days(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

async fn query_period(token: &str, account_id: &str, date: &str) -> Result<UsagePeriod> {
    let start_time = format!("{date}T00:00:00Z");
    let end_time = format!("{date}T23:59:59Z");
    let body = serde_json::to_string(&GraphQlRequest {
        query: USAGE_QUERY,
        variables: GraphQlVariables {
            account_tag: account_id,
            start: date,
            end: date,
            start_time: &start_time,
            end_time: &end_time,
        },
    })?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(JsValue::from_str(&body)));
    let request = Request::new_with_init(GRAPHQL_URL, &init)?;
    request
        .headers()
        .set("Authorization", &format!("Bearer {token}"))?;
    request.headers().set("Content-Type", "application/json")?;
    request.headers().set("Accept", "application/json")?;

    let mut response = Fetch::Request(request).send().await?;
    let status = response.status_code();
    let payload: GraphQlResponse = response.json().await?;
    if !(200..300).contains(&status) || !payload.errors.is_empty() {
        let message = payload
            .errors
            .into_iter()
            .map(|value| value.message)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(worker::Error::RustError(if message.is_empty() {
            format!("Cloudflare GraphQL HTTP {status}")
        } else {
            message
        }));
    }
    let account = payload
        .data
        .and_then(|value| value.viewer.accounts.into_iter().next())
        .ok_or_else(|| worker::Error::RustError("Cloudflare 账户不可用".to_string()))?;
    let (rows_read, rows_written) = account.d1_analytics_adaptive_groups.into_iter().fold(
        (0_i64, 0_i64),
        |(read, written), group| {
            (
                read.saturating_add(group.sum.rows_read),
                written.saturating_add(group.sum.rows_written),
            )
        },
    );
    let workers_requests = account
        .workers_invocations_adaptive
        .into_iter()
        .fold(0_i64, |total, group| {
            total.saturating_add(group.sum.requests)
        });
    Ok(UsagePeriod {
        rows_read,
        rows_written,
        workers_requests,
    })
}

pub async fn usage(token: &str, account_id: &str, now: i64) -> Result<CloudflareUsage> {
    let today = date_from_unix_days(now.div_euclid(86_400));
    let yesterday = date_from_unix_days(now.div_euclid(86_400) - 1);
    let today_usage = query_period(token, account_id, &today).await?;
    let yesterday_usage = query_period(token, account_id, &yesterday).await?;
    Ok(CloudflareUsage {
        today: today_usage,
        yesterday: yesterday_usage,
    })
}

#[cfg(test)]
mod tests {
    use super::date_from_unix_days;

    #[test]
    fn converts_unix_days_to_utc_date() {
        assert_eq!(date_from_unix_days(0), "1970-01-01");
        assert_eq!(date_from_unix_days(20_673), "2026-08-08");
    }
}
