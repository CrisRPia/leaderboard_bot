use anyhow::{Result, anyhow};
use chrono::{DateTime, Days, Utc};
use itertools::Itertools;
use std::{collections::HashMap, future, sync::OnceLock};

use poise::serenity_prelude::{
    self as serenity, Message,
    futures::{StreamExt, TryStreamExt, stream::BoxStream},
};
use regex::Regex;
pub struct Data {}
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

#[derive(Debug, PartialEq, PartialOrd, Ord, Eq, Hash, Clone)]
enum User {
    Id(serenity::UserId),
    Text(String),
}

impl User {
    fn mention(&self) -> String {
        match self {
            Self::Id(id) => format!("<@{}>", id.get()),
            Self::Text(name) => format!("@{}", name),
        }
    }
}

#[derive(Debug, Clone)]
struct LeaderboardMessageData {
    score: Option<i32>,
    user: User,
}

struct UserStats {
    user: User,
    winrate: f64,
    games: i32,
    wins: i32,
    avg: f64,
}

fn parse_line(line: &str) -> Vec<LeaderboardMessageData> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?P<val>[0-9X])/6: (?P<users>.*)").unwrap());

    if let Some(caps) = re.captures(line) {
        let val_str = &caps["val"];
        let score = if val_str == "X" {
            None
        } else {
            val_str.parse::<i32>().ok()
        };

        let users_block = &caps["users"];

        return users_block
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .filter_map(|u| {
                let clean_name = u.trim_matches(|c| r#"<>@!,*""#.contains(c));

                if let Some(id) = clean_name.parse::<serenity::UserId>().ok() {
                    return Some(LeaderboardMessageData {
                        user: User::Id(id),
                        score,
                    });
                }

                return Some(LeaderboardMessageData {
                    user: User::Text(clean_name.to_string()),
                    score,
                });
            })
            .collect();
    }

    return vec![];
}

pub fn determine_user(users: &[serenity::Member], username: &str) -> Option<serenity::UserId> {
    users
        .iter()
        .find_or_first(|u| u.user.name == username || u.nick == Some(username.to_string()))
        .map(|u| u.user.id)
}

pub fn parse_dates(
    _ctx: Context<'_>,
    from: &str,
    to: &str,
) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let from = chrono_english::parse_date_string(from, Utc::now(), chrono_english::Dialect::Uk)?;
    let to = chrono_english::parse_date_string(to, Utc::now(), chrono_english::Dialect::Uk)?;

    if from >= to {
        return Err(anyhow!("{} is not prior to {}", from, to));
    }

    return Ok((from, to));
}

// This assumes messages come from newest to oldest.
pub async fn get_messages_from_dates(
    ctx: Context<'_>,
    target_channel_id: serenity::ChannelId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> BoxStream<'_, Result<Message>> {
    target_channel_id
        .messages_iter(ctx)
        // We add 1 day because we're actually reading "results from yesterday" tables.
        .try_filter(move |msg| {
            future::ready(
                msg.timestamp.timestamp() <= to.checked_add_days(Days::new(1)).unwrap().timestamp(),
            )
        })
        .try_take_while(move |msg| {
            future::ready(Ok(msg.timestamp.timestamp()
                >= from.checked_add_days(Days::new(1)).unwrap().timestamp()))
        })
        .map_err(|err| anyhow!(err))
        .boxed()
}

#[poise::command(slash_command, prefix_command)]
pub async fn leaderboard(
    ctx: Context<'_>,
    #[description = "Channel to read history from"] channel: Option<serenity::GuildChannel>,
    #[description = "Starting date. Default: sunday"] from: Option<String>,
    #[description = "End date. Default: today"] to: Option<String>,
) -> Result<(), Error> {
    ctx.defer().await?;

    let from = from.unwrap_or("sunday".to_string());
    let to = to.unwrap_or("today".to_string());

    dbg!(&from, &to);
    let (from_date, to_date) = parse_dates(ctx, &from, &to)?;
    dbg!(&from_date, &to);

    let target_channel_id = channel.map(|c| c.id).unwrap_or(ctx.channel_id());
    let guild_id = ctx.guild_id().ok_or("Must be run in a server")?;

    let messages_within_range =
        get_messages_from_dates(ctx, target_channel_id, from_date, to_date).await;

    let result = messages_within_range
        .map(|m| match m {
            Err(err) => {
                dbg!(err);
                return None;
            }
            Ok(message) => {
                if !message.author.bot {
                    return None;
                }

                let captures = message
                    .content
                    .lines()
                    .map(parse_line)
                    .flatten()
                    .collect::<Vec<_>>();

                return Some(captures);
            }
        })
        .collect::<Vec<_>>()
        .await;

    let games = result
        .iter()
        .filter_map(|v| v.as_ref())
        .filter(|v| !v.is_empty())
        .flatten()
        .collect::<Vec<_>>();

    let members = guild_id
        .members_iter(ctx)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect_vec();

    let resolved_games: Vec<LeaderboardMessageData> = games
        .into_iter()
        .map(|game| {
            let mut game = game.clone();
            if let User::Text(name) = &game.user {
                if let Some(found_id) = determine_user(&members, name) {
                    game.user = User::Id(found_id);
                }
            }
            game
        })
        .collect();

    // 2. Process groups (Calculate stats)
    let mut groups: HashMap<&User, Vec<&LeaderboardMessageData>> = HashMap::new();

    for msg in &resolved_games {
        groups.entry(&msg.user).or_default().push(msg);
    }

    let stats: Vec<_> = groups
        .into_iter()
        .map(|(user, messages)| {
            let games = messages.len();
            let games_with_score = messages.iter().filter_map(|m| m.score).collect_vec();
            let wins = games_with_score.len();
            let avg = games_with_score.iter().sum::<i32>() as f64 / games_with_score.len() as f64;

            let win_rate = if games > 0 {
                wins as f64 / games as f64
            } else {
                0.0
            };

            UserStats {
                user: user.clone(),
                games: games as i32,
                winrate: win_rate,
                wins: wins as i32,
                avg,
            }
        })
        .collect();

    let rows = stats
        .iter()
        .sorted_by(|a, b| b.winrate.partial_cmp(&a.winrate).unwrap())
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}. {} — **{:.0}%** ({} wins / {} games; {} avg score)",
                i + 1,
                s.user.mention(),
                s.winrate * 100.0,
                s.wins,
                s.games,
                s.avg,
            )
        })
        .join("\n");

    let mut output = format!(
        "## 🏆 Wordle Leaderboard\n > From {} ({}) to before {} ({})\n {}",
        from, from_date.format("%Y/%m/%d %H:%M"),
        to, to_date.format("%Y/%m/%d %H:%M"),
        rows
    );

    if output.len() > 1950 {
        output.truncate(1950);
        output.push_str("\n... (truncated)");
    }

    ctx.send(
        poise::CreateReply::default()
            .content(output)
            .allowed_mentions(serenity::CreateAllowedMentions::new().empty_users()),
    )
    .await?;

    Ok(())
}
