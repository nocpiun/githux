//! Contribution heatmap via `contributionsCollection.contributionCalendar`.
//!
//! Reads a caller-supplied, validated date range. Only server-visible
//! contribution data is returned: private contribution details are never
//! exposed by GitHub here, and this adapter does not try to infer or guess
//! hidden contributions.

use crate::auth::token::Pat;
use crate::domain::profile::{
    ContributionCalendar, ContributionDay, ContributionMonth, ContributionRange, ContributionWeek,
    ProfileError, ProfileGraphqlError,
};
use crate::github::client::GitHubClient;
use crate::github::profile::graphql::{ProfileLimits, require_user, run_query};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;

const CONTRIBUTIONS_QUERY: &str = r#"
query ProfileContributions($login: String!, $from: DateTime!, $to: DateTime!) {
  user(login: $login) {
    contributionsCollection(from: $from, to: $to) {
      contributionCalendar {
        totalContributions
        weeks {
          contributionDays {
            date
            contributionCount
            contributionLevel
            color
          }
        }
        months {
          name
          year
          firstDay
          totalWeeks
        }
      }
    }
  }
}
"#;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContributionsUser {
    #[serde(default)]
    contributions_collection: Option<ContributionsCollection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContributionsCollection {
    #[serde(default)]
    contribution_calendar: Option<ContributionCalendarNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContributionCalendarNode {
    #[serde(default)]
    total_contributions: u64,
    #[serde(default)]
    weeks: Vec<WeekNode>,
    #[serde(default)]
    months: Vec<MonthNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WeekNode {
    #[serde(default)]
    contribution_days: Vec<DayNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DayNode {
    #[serde(default)]
    date: String,
    #[serde(default)]
    contribution_count: u64,
    #[serde(default)]
    contribution_level: String,
    #[serde(default)]
    color: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonthNode {
    #[serde(default)]
    name: String,
    #[serde(default)]
    year: u32,
    #[serde(default)]
    first_day: String,
    #[serde(default)]
    total_weeks: u32,
}

/// Fetch the contribution calendar for a login over a validated range.
pub async fn fetch_contribution_calendar(
    client: &GitHubClient,
    token: &Pat,
    login: &str,
    range: &ContributionRange,
    limits: &ProfileLimits,
    cancel: CancellationToken,
) -> Result<ContributionCalendar, ProfileError> {
    crate::github::profile::validate_login(login)?;
    let variables = serde_json::json!({
        "login": login,
        "from": range.start_datetime(),
        "to": range.end_datetime(),
    });
    let outcome = run_query(
        client,
        token,
        CONTRIBUTIONS_QUERY,
        &variables,
        limits,
        cancel,
    )
    .await?;
    let (user, errors) = require_user::<ContributionsUser>(&outcome)?;
    let collection = user
        .contributions_collection
        .ok_or_else(|| unavailable(&errors))?;
    let calendar = collection
        .contribution_calendar
        .ok_or_else(|| unavailable(&errors))?;
    if calendar.weeks.len() > limits.max_weeks {
        return Err(ProfileError::LimitExceeded {
            resource: "contribution weeks",
            limit: limits.max_weeks,
        });
    }
    let weeks = calendar
        .weeks
        .into_iter()
        .map(|week| ContributionWeek {
            days: week
                .contribution_days
                .into_iter()
                .map(|day| ContributionDay {
                    date: day.date,
                    count: day.contribution_count,
                    level: day.contribution_level,
                    color: day.color,
                })
                .collect(),
        })
        .collect();
    let months = calendar
        .months
        .into_iter()
        .map(|month| ContributionMonth {
            name: month.name,
            year: month.year,
            first_day: month.first_day,
            total_weeks: month.total_weeks,
        })
        .collect();
    Ok(ContributionCalendar {
        weeks,
        months,
        total_contributions: calendar.total_contributions,
        errors,
    })
}

fn unavailable(errors: &[ProfileGraphqlError]) -> ProfileError {
    match errors.first() {
        Some(error) => ProfileError::Graphql {
            kind: error.kind.clone(),
            path: error.path.clone(),
        },
        None => ProfileError::InvalidResponse {
            message: "contribution calendar unavailable".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::profile::GraphqlErrorKind;
    use crate::network::transport::{MockTransport, json_response};
    use std::sync::Arc;

    fn client(mock: Arc<MockTransport>) -> GitHubClient {
        GitHubClient::new(Default::default(), mock)
    }

    async fn enqueue(mock: &MockTransport, body: serde_json::Value) {
        mock.enqueue(Ok(json_response(200, &body.to_string())))
            .await;
    }

    fn calendar_body(total: u64, days: &[(u64, &str)]) -> serde_json::Value {
        let week_days: Vec<serde_json::Value> = days
            .iter()
            .map(|(count, level)| {
                serde_json::json!({
                    "date": "2024-01-01",
                    "contributionCount": count,
                    "contributionLevel": level,
                    "color": "#ebedf0"
                })
            })
            .collect();
        serde_json::json!({
            "data": {"user": {"contributionsCollection": {"contributionCalendar": {
                "totalContributions": total,
                "weeks": [{"contributionDays": week_days}],
                "months": [{"name": "Jan", "year": 2024, "firstDay": "2024-01-01", "totalWeeks": 1}]
            }}}}
        })
    }

    #[tokio::test]
    async fn parses_calendar_fields() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            calendar_body(3, &[(2, "FIRST_QUARTILE"), (1, "NONE")]),
        )
        .await;
        let range = ContributionRange::new("2024-01-01", "2024-01-07").unwrap();
        let calendar = fetch_contribution_calendar(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &range,
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("calendar");
        assert_eq!(calendar.total_contributions, 3);
        assert_eq!(calendar.weeks.len(), 1);
        assert_eq!(calendar.weeks[0].days.len(), 2);
        assert_eq!(calendar.weeks[0].days[0].count, 2);
        assert_eq!(calendar.weeks[0].days[0].level, "FIRST_QUARTILE");
        assert_eq!(calendar.weeks[0].days[0].color, "#ebedf0");
        assert_eq!(calendar.months[0].name, "Jan");
    }

    /// A genuinely zero-contribution calendar is valid data, not an error.
    #[tokio::test]
    async fn zero_contributions_is_a_valid_calendar() {
        let mock = Arc::new(MockTransport::new());
        enqueue(&mock, calendar_body(0, &[(0, "NONE")])).await;
        let range = ContributionRange::new("2024-01-01", "2024-01-07").unwrap();
        let calendar = fetch_contribution_calendar(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &range,
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("calendar");
        assert_eq!(calendar.total_contributions, 0);
        assert_eq!(calendar.weeks[0].days[0].count, 0);
        assert_eq!(calendar.weeks[0].days[0].level, "NONE");
    }

    /// An unknown `contributionLevel` from a schema change is preserved.
    #[tokio::test]
    async fn unknown_level_survives() {
        let mock = Arc::new(MockTransport::new());
        enqueue(&mock, calendar_body(1, &[(1, "FIFTH_QUARTILE")])).await;
        let range = ContributionRange::new("2024-01-01", "2024-01-07").unwrap();
        let calendar = fetch_contribution_calendar(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &range,
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("calendar");
        assert_eq!(calendar.weeks[0].days[0].level, "FIFTH_QUARTILE");
    }

    /// Insufficient permission (GraphQL FORBIDDEN) is a typed error, not an
    /// empty calendar.
    #[tokio::test]
    async fn permission_denied_is_a_typed_error() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({
                "data": {"user": {"contributionsCollection": null}},
                "errors": [{
                    "type": "FORBIDDEN",
                    "path": ["user", "contributionsCollection"],
                    "message": "Resource not accessible"
                }]
            }),
        )
        .await;
        let range = ContributionRange::new("2024-01-01", "2024-01-07").unwrap();
        let result = fetch_contribution_calendar(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &range,
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ProfileError::Graphql {
                kind: GraphqlErrorKind::Forbidden,
                ..
            })
        ));
    }

    /// GraphQL primary rate limiting (200 + remaining 0) is a typed error.
    #[tokio::test]
    async fn rate_limited_is_a_typed_error() {
        let mock = Arc::new(MockTransport::new());
        let mut response = json_response(
            200,
            &serde_json::json!({
                "data": null,
                "errors": [{"type": "RATE_LIMITED", "path": ["user"], "message": "rate limit"}]
            })
            .to_string(),
        );
        // Overwrite the seeded remaining header so the primary-limit signal is
        // unambiguous.
        for (name, value) in response.headers.iter_mut() {
            if name == "x-ratelimit-remaining" {
                *value = "0".to_string();
            }
        }
        mock.enqueue(Ok(response)).await;
        let range = ContributionRange::new("2024-01-01", "2024-01-07").unwrap();
        let result = fetch_contribution_calendar(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &range,
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ProfileError::Graphql {
                kind: GraphqlErrorKind::RateLimitPrimary,
                ..
            })
        ));
    }

    /// The query sends the validated range as GraphQL variables.
    #[tokio::test]
    async fn range_is_sent_as_variables() {
        let mock = Arc::new(MockTransport::new());
        enqueue(&mock, calendar_body(0, &[])).await;
        let range = ContributionRange::new("2024-01-01", "2024-01-07").unwrap();
        let _ = fetch_contribution_calendar(
            &client(mock.clone()),
            &Pat::from_str("ghp_test"),
            "octocat",
            &range,
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("calendar");
        let recorded = mock.recorded_requests().await;
        let body: serde_json::Value =
            serde_json::from_str(recorded[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["variables"]["from"], "2024-01-01T00:00:00Z");
        assert_eq!(body["variables"]["to"], "2024-01-07T23:59:59Z");
        assert!(
            body["query"]
                .as_str()
                .unwrap()
                .contains("contributionCalendar")
        );
    }
}
