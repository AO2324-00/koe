use crate::connection_status::{VoiceConnectionStatus, VoiceConnectionStatusMap};
use crate::context_store;
use crate::sanitize::sanitize_response;
use crate::speech::{NewSpeechQueueOption, SpeechQueue};
use crate::voice_client::VoiceClient;
use anyhow::{Context as _, Result};
use koe_db::dict::{GetAllOption, InsertOption, InsertResponse, RemoveOption, RemoveResponse};
use koe_db::redis;
use koe_db::voice::{SetKindOption, SetPitchOption, SetSpeedOption};
use koe_speech::SpeechProvider;
use log::error;
use serenity::builder::CreateEmbed;
use serenity::{
    client::Context,
    model::{
        id::{ChannelId, GuildId, UserId},
        interactions::{
            application_command::{
                ApplicationCommandInteraction, ApplicationCommandInteractionDataOptionValue,
            },
            InteractionResponseType,
        },
    },
};

#[derive(Debug, Clone)]
enum CommandKind {
    Join,
    Leave,
    DictAdd(DictAddOption),
    DictRemove(DictRemoveOption),
    DictView,
    VoiceKind(VoiceKindOption),
    VoiceSpeed(VoiceSpeedOption),
    VoicePitch(VoicePitchOption),
    Help,
    Unknown,
}

#[derive(Debug, Clone)]
struct DictAddOption {
    pub word: String,
    pub read_as: String,
}

#[derive(Debug, Clone)]
struct DictRemoveOption {
    pub word: String,
}

#[derive(Debug, Clone)]
struct VoiceKindOption {
    pub kind: String,
}

#[derive(Debug, Clone)]
struct VoiceSpeedOption {
    pub speed: f64,
}

#[derive(Debug, Clone)]
struct VoicePitchOption {
    pub pitch: f64,
}

#[derive(Debug, Clone)]
enum CommandResponse {
    Text(String),
    Embed(CreateEmbed),
}

impl<T> From<T> for CommandResponse
where
    T: ToString,
{
    fn from(value: T) -> Self {
        CommandResponse::Text(value.to_string())
    }
}

impl From<&ApplicationCommandInteraction> for CommandKind {
    fn from(cmd: &ApplicationCommandInteraction) -> Self {
        match cmd.data.name.as_str() {
            "join" | "kjoin" => CommandKind::Join,
            "leave" | "kleave" => CommandKind::Leave,
            "dict" => {
                let option_dict = match cmd.data.options.get(0) {
                    Some(option) => option,
                    None => return CommandKind::Unknown,
                };

                match option_dict.name.as_str() {
                    "add" => {
                        let option_word = match option_dict.options.get(0) {
                            Some(x) => x,
                            None => return CommandKind::Unknown,
                        };
                        let option_read_as = match option_dict.options.get(1) {
                            Some(x) => x,
                            None => return CommandKind::Unknown,
                        };
                        let word = match &option_word.resolved {
                            Some(ApplicationCommandInteractionDataOptionValue::String(x)) => x,
                            _ => return CommandKind::Unknown,
                        };
                        let read_as = match &option_read_as.resolved {
                            Some(ApplicationCommandInteractionDataOptionValue::String(x)) => x,
                            _ => return CommandKind::Unknown,
                        };

                        CommandKind::DictAdd(DictAddOption {
                            word: word.clone(),
                            read_as: read_as.clone(),
                        })
                    }
                    "remove" => {
                        let option_word = match option_dict.options.get(0) {
                            Some(x) => x,
                            None => return CommandKind::Unknown,
                        };
                        let word = match &option_word.resolved {
                            Some(ApplicationCommandInteractionDataOptionValue::String(x)) => x,
                            _ => return CommandKind::Unknown,
                        };

                        CommandKind::DictRemove(DictRemoveOption { word: word.clone() })
                    }
                    "view" => CommandKind::DictView,
                    _ => CommandKind::Unknown,
                }
            }
            "voice" => {
                let option_voice = match cmd.data.options.get(0) {
                    Some(option) => option,
                    None => return CommandKind::Unknown,
                };

                match option_voice.name.as_str() {
                    "kind" => {
                        let option_kind = match option_voice.options.get(0) {
                            Some(x) => x,
                            None => return CommandKind::Unknown,
                        };
                        let kind = match &option_kind.resolved {
                            Some(ApplicationCommandInteractionDataOptionValue::String(x)) => x,
                            _ => return CommandKind::Unknown,
                        };

                        CommandKind::VoiceKind(VoiceKindOption { kind: kind.clone() })
                    }
                    "speed" => {
                        let option_speed = match option_voice.options.get(0) {
                            Some(x) => x,
                            None => return CommandKind::Unknown,
                        };
                        let speed = match &option_speed.resolved {
                            Some(ApplicationCommandInteractionDataOptionValue::Number(x)) => x,
                            _ => return CommandKind::Unknown,
                        };

                        CommandKind::VoiceSpeed(VoiceSpeedOption { speed: *speed })
                    }
                    "pitch" => {
                        let option_pitch = match option_voice.options.get(0) {
                            Some(x) => x,
                            None => return CommandKind::Unknown,
                        };
                        let pitch = match &option_pitch.resolved {
                            Some(ApplicationCommandInteractionDataOptionValue::Number(x)) => x,
                            _ => return CommandKind::Unknown,
                        };

                        CommandKind::VoicePitch(VoicePitchOption { pitch: *pitch })
                    }
                    _ => CommandKind::Unknown,
                }
            }
            "help" => CommandKind::Help,
            _ => CommandKind::Unknown,
        }
    }
}

pub async fn handle_command(ctx: &Context, command: &ApplicationCommandInteraction) -> Result<()> {
    let response = execute_command(ctx, command).await;

    command
        .create_interaction_response(&ctx.http, |create_response| {
            create_response
                .kind(InteractionResponseType::ChannelMessageWithSource)
                .interaction_response_data(|create_message| match response {
                    CommandResponse::Text(text) => create_message.content(text),
                    CommandResponse::Embed(embed) => create_message.add_embed(embed),
                })
        })
        .await
        .context("Failed to create interaction response")?;

    Ok(())
}

async fn execute_command(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
) -> CommandResponse {
    let command_kind = CommandKind::from(command);

    let res = match command_kind {
        CommandKind::Join => handle_join(ctx, command).await,
        CommandKind::Leave => handle_leave(ctx, command).await,
        CommandKind::DictAdd(option) => handle_dict_add(ctx, command, option).await,
        CommandKind::DictRemove(option) => handle_dict_remove(ctx, command, option).await,
        CommandKind::DictView => handle_dict_view(ctx, command).await,
        CommandKind::VoiceKind(option) => handle_voice_kind(ctx, command, option).await,
        CommandKind::VoiceSpeed(option) => handle_voice_speed(ctx, command, option).await,
        CommandKind::VoicePitch(option) => handle_voice_pitch(ctx, command, option).await,
        CommandKind::Help => handle_help(ctx, command).await,
        CommandKind::Unknown => {
            error!("Failed to parse command: {:?}", command);
            Ok("エラー: コマンドを認識できません。".into())
        }
    };

    match res {
        Ok(message) => message,
        Err(err) => {
            error!("Error while executing command: {}", err);
            "内部エラーが発生しました。".into()
        }
    }
}

async fn handle_join(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
) -> Result<CommandResponse> {
    let guild_id = match command.guild_id {
        Some(id) => id,
        None => return Ok("`/join`, `/kjoin` はサーバー内でのみ使えます。".into()),
    };
    let user_id = command.user.id;
    let text_channel_id = command.channel_id;

    let voice_channel_id = match get_user_voice_channel(ctx, &guild_id, &user_id).await? {
        Some(channel) => channel,
        None => {
            return Ok("ボイスチャンネルに接続してから `/join` を送信してください。".into());
        }
    };

    let voice_client = context_store::extract::<VoiceClient>(ctx).await?;
    let call = voice_client.join(ctx, guild_id, voice_channel_id).await?;

    let speech_provider = context_store::extract::<SpeechProvider>(ctx).await?;

    let status_map = context_store::extract::<VoiceConnectionStatusMap>(ctx).await?;
    status_map.insert(
        guild_id,
        VoiceConnectionStatus {
            bound_text_channel: text_channel_id,
            last_message_read: None,
            speech_queue: SpeechQueue::new(NewSpeechQueueOption {
                guild_id,
                speech_provider,
                call,
            }),
        },
    );

    Ok("接続しました。".into())
}

async fn handle_leave(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
) -> Result<CommandResponse> {
    let guild_id = match command.guild_id {
        Some(id) => id,
        None => return Ok("`/leave`, `/kleave` はサーバー内でのみ使えます。".into()),
    };

    let voice_client = context_store::extract::<VoiceClient>(ctx).await?;

    if !voice_client.is_connected(ctx, guild_id).await? {
        return Ok("どのボイスチャンネルにも接続していません。".into());
    }

    voice_client.leave(ctx, guild_id).await?;

    let status_map = context_store::extract::<VoiceConnectionStatusMap>(ctx).await?;
    status_map.remove(&guild_id);

    Ok("切断しました。".into())
}

async fn handle_dict_add(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
    option: DictAddOption,
) -> Result<CommandResponse> {
    let guild_id = match command.guild_id {
        Some(id) => id,
        None => return Ok("`/dict add` はサーバー内でのみ使えます。".into()),
    };

    let client = context_store::extract::<redis::Client>(ctx).await?;
    let mut conn = client.get_async_connection().await?;

    let resp = koe_db::dict::insert(
        &mut conn,
        InsertOption {
            guild_id: guild_id.to_string(),
            word: option.word.clone(),
            read_as: option.read_as.clone(),
        },
    )
    .await?;

    match resp {
        InsertResponse::Success => Ok(format!(
            "{}の読み方を{}として辞書に登録しました。",
            sanitize_response(&option.word),
            sanitize_response(&option.read_as)
        )
        .into()),
        InsertResponse::WordAlreadyExists => Ok(format!(
            "すでに{}は辞書に登録されています。",
            sanitize_response(&option.word)
        )
        .into()),
    }
}

async fn handle_dict_remove(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
    option: DictRemoveOption,
) -> Result<CommandResponse> {
    let guild_id = match command.guild_id {
        Some(id) => id,
        None => return Ok("`/dict remove` はサーバー内でのみ使えます。".into()),
    };

    let client = context_store::extract::<redis::Client>(ctx).await?;
    let mut conn = client.get_async_connection().await?;

    let resp = koe_db::dict::remove(
        &mut conn,
        RemoveOption {
            guild_id: guild_id.to_string(),
            word: option.word.clone(),
        },
    )
    .await?;

    match resp {
        RemoveResponse::Success => Ok(format!(
            "辞書から{}を削除しました。",
            sanitize_response(&option.word)
        )
        .into()),
        RemoveResponse::WordDoesNotExist => Ok(format!(
            "{}は辞書に登録されていません。",
            sanitize_response(&option.word)
        )
        .into()),
    }
}

async fn handle_dict_view(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
) -> Result<CommandResponse> {
    let guild_id = match command.guild_id {
        Some(id) => id,
        None => return Ok("`/dict view` はサーバー内でのみ使えます。".into()),
    };

    let client = context_store::extract::<redis::Client>(ctx).await?;
    let mut conn = client.get_async_connection().await?;

    let dict = koe_db::dict::get_all(
        &mut conn,
        GetAllOption {
            guild_id: guild_id.to_string(),
        },
    )
    .await?;

    let mut embed = CreateEmbed::default();

    let guild_name = guild_id
        .name(&ctx.cache)
        .await
        .unwrap_or_else(|| "サーバー".to_string());
    embed.title(format!("📕 {}の辞書", guild_name));

    embed.fields(
        dict.into_iter()
            .map(|(word, read_as)| (word, sanitize_response(&read_as), false)),
    );

    Ok(CommandResponse::Embed(embed))
}

async fn handle_voice_kind(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
    option: VoiceKindOption,
) -> Result<CommandResponse> {
    let client = context_store::extract::<redis::Client>(ctx).await?;
    let mut conn = client.get_async_connection().await?;

    match option.kind.as_str() {
        "A" | "B" | "C" | "D" => {}
        _ => return Ok("声の種類を認識できません。".into()),
    }

    koe_db::voice::set_kind(
        &mut conn,
        SetKindOption {
            user_id: command.user.id.to_string(),
            kind: option.kind.clone(),
        },
    )
    .await?;

    Ok(format!(
        "声の種類を{}に設定しました。",
        sanitize_response(&option.kind)
    )
    .into())
}

async fn handle_voice_speed(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
    option: VoiceSpeedOption,
) -> Result<CommandResponse> {
    let client = context_store::extract::<redis::Client>(ctx).await?;
    let mut conn = client.get_async_connection().await?;

    if option.speed < 0.25 || 4.0 < option.speed {
        return Ok("速度は 0.25 以上 4.0 以下の値を指定してください。".into());
    }

    koe_db::voice::set_speed(
        &mut conn,
        SetSpeedOption {
            user_id: command.user.id.to_string(),
            speed: option.speed,
        },
    )
    .await?;

    Ok(format!("声の速度を{}に設定しました。", option.speed).into())
}

async fn handle_voice_pitch(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
    option: VoicePitchOption,
) -> Result<CommandResponse> {
    let client = context_store::extract::<redis::Client>(ctx).await?;
    let mut conn = client.get_async_connection().await?;

    if option.pitch < -20.0 || 20.0 < option.pitch {
        return Ok("ピッチは -20.0 以上 20.0 以下の値を指定してください。".into());
    }

    koe_db::voice::set_pitch(
        &mut conn,
        SetPitchOption {
            user_id: command.user.id.to_string(),
            pitch: option.pitch,
        },
    )
    .await?;

    Ok(format!("声のピッチを{}に設定しました。", option.pitch).into())
}

async fn handle_help(
    _ctx: &Context,
    _command: &ApplicationCommandInteraction,
) -> Result<CommandResponse> {
    Ok("使い方はこちらをご覧ください:\nhttps://github.com/ciffelia/koe/blob/main/README.md".into())
}

async fn get_user_voice_channel(
    ctx: &Context,
    guild_id: &GuildId,
    user_id: &UserId,
) -> Result<Option<ChannelId>> {
    let guild = guild_id
        .to_guild_cached(&ctx.cache)
        .await
        .context("Failed to find guild in the cache")?;

    let channel_id = guild
        .voice_states
        .get(user_id)
        .and_then(|voice_state| voice_state.channel_id);

    Ok(channel_id)
}
