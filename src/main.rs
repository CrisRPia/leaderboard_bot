use chrono::Utc;
use dotenvy::dotenv;
use poise::serenity_prelude as serenity;
use serde::Deserialize;

use wordle_stats_bot::*;

#[derive(Deserialize, Debug)]
struct Config {
    discord_token: String,
}

#[tokio::main]
async fn main() {
    dbg!(dotenv().is_ok());
    let config = envy::from_env::<Config>().unwrap();
    let token = config.discord_token;

    let intents = serenity::GatewayIntents::non_privileged()
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILD_MEMBERS;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![leaderboard()],
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                let guild_id = serenity::GuildId::new(794733579770920990);
                poise::builtins::register_in_guild(ctx, &framework.options().commands, guild_id)
                    .await?;
                // poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                println!("Bot started.");
                Ok(Data {})
            })
        })
        .build();

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;
    client.unwrap().start().await.unwrap();
}
