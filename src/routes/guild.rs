use super::channel::ChannelType;
use super::Response;
use crate::database::guild::{fetch_member as get_member, get_invite, Guild, MemberKey};
use crate::database::{
    self, channel::fetch_channel, channel::Channel, guild::serialise_guilds_with_channels,
    user::User, Permission, PermissionCalculator,
};
use crate::util::gen_token;

use mongodb::bson::{doc, Bson};
use mongodb::options::{FindOneOptions, FindOptions};
use rocket::request::Form;
use rocket_contrib::json::Json;
use serde::{Deserialize, Serialize};
use rocket::futures::StreamExt;
use ulid::Ulid;

// ! FIXME: GET RID OF THIS
macro_rules! with_permissions {
    ($user: expr, $target: expr) => {{
        let permissions = PermissionCalculator::new($user.clone())
            .guild($target.clone())
            .fetch_data()
            .await;

        let value = permissions.as_permission().await;
        if !value.get_access() {
            return None;
        }

        (value, permissions.member.unwrap())
    }};
}

/// fetch your guilds
#[get("/@me")]
pub async fn my_guilds(user: User) -> Response {
    if let Ok(gids) = user.find_guilds().await {
        if let Ok(data) = serialise_guilds_with_channels(&gids).await {
            Response::Success(json!(data))
        } else {
            Response::InternalServerError(json!({ "error": "Failed to fetch guilds." }))
        }
    } else {
        Response::InternalServerError(json!({ "error": "Failed to fetch memberships." }))
    }
}

/// fetch a guild
#[get("/<target>")]
pub async fn guild(user: User, target: Guild) -> Option<Response> {
    with_permissions!(user, target);

    if let Ok(result) = target.seralise_with_channels().await {
        Some(Response::Success(result))
    } else {
        Some(Response::InternalServerError(
            json!({ "error": "Failed to fetch channels!" }),
        ))
    }
}

/// delete or leave a guild
#[delete("/<target>")]
pub async fn remove_guild(user: User, target: Guild) -> Option<Response> {
    with_permissions!(user, target);

    if user.id == target.owner {
        let channels = database::get_collection("channels");
        if let Ok(mut result) = channels.find(
            doc! {
                "type": 2,
                "guild": &target.id
            },
            FindOptions::builder().projection(doc! { "_id": 1 }).build(),
        ).await {
            let mut values = vec![];
            while let Some(item) = result.next().await {
                if let Ok(doc) = item {
                    values.push(Bson::String(doc.get_str("_id").unwrap().to_string()));
                }
            }

            if database::get_collection("messages")
                .delete_many(
                    doc! {
                        "channel": {
                            "$in": values
                        }
                    },
                    None,
                )
                .await
                .is_ok()
            {
                if channels
                    .delete_many(
                        doc! {
                            "type": 2,
                            "guild": &target.id,
                        },
                        None,
                    )
                    .await
                    .is_ok()
                {
                    if database::get_collection("members")
                        .delete_many(
                            doc! {
                                "_id.guild": &target.id,
                            },
                            None,
                        )
                        .await
                        .is_ok()
                    {
                        if database::get_collection("guilds")
                            .delete_one(
                                doc! {
                                    "_id": &target.id
                                },
                                None,
                            )
                            .await
                            .is_ok()
                        {
                            /*notifications::send_message_threaded(
                                None,
                                target.id.clone(),
                                Notification::guild_delete(Delete {
                                    id: target.id.clone(),
                                }), FIXME
                            );*/

                            Some(Response::Result(super::Status::Ok))
                        } else {
                            Some(Response::InternalServerError(
                                json!({ "error": "Failed to delete guild." }),
                            ))
                        }
                    } else {
                        Some(Response::InternalServerError(
                            json!({ "error": "Failed to delete guild members." }),
                        ))
                    }
                } else {
                    Some(Response::InternalServerError(
                        json!({ "error": "Failed to delete guild channels." }),
                    ))
                }
            } else {
                Some(Response::InternalServerError(
                    json!({ "error": "Failed to delete guild messages." }),
                ))
            }
        } else {
            Some(Response::InternalServerError(
                json!({ "error": "Could not fetch channels." }),
            ))
        }
    } else if database::get_collection("members")
        .delete_one(
            doc! {
                "_id.guild": &target.id,
                "_id.user": &user.id,
            },
            None,
        )
        .await
        .is_ok()
    {
        /*notifications::send_message_threaded(
            None,
            target.id.clone(),
            Notification::guild_user_leave(UserLeave {
                id: target.id.clone(),
                user: user.id.clone(),
                banned: false,
            }), FIXME
        );*/

        Some(Response::Result(super::Status::Ok))
    } else {
        Some(Response::InternalServerError(
            json!({ "error": "Failed to remove you from the guild." }),
        ))
    }
}

#[derive(Serialize, Deserialize)]
pub struct CreateChannel {
    nonce: String,
    name: String,
    description: Option<String>,
}

/// create a new channel
#[post("/<target>/channels", data = "<info>")]
pub async fn create_channel(user: User, target: Guild, info: Json<CreateChannel>) -> Option<Response> {
    let (permissions, _) = with_permissions!(user, target);

    if !permissions.get_manage_channels() {
        return Some(Response::LackingPermission(Permission::ManageChannels));
    }

    let nonce: String = info.nonce.chars().take(32).collect();
    let name: String = info.name.chars().take(32).collect();
    let description: String = info
        .description
        .clone()
        .unwrap_or(String::new())
        .chars()
        .take(255)
        .collect();

    if let Ok(result) =
        database::get_collection("channels").find_one(doc! { "nonce": &nonce }, None)
        .await
    {
        if result.is_some() {
            return Some(Response::BadRequest(
                json!({ "error": "Channel already created." }),
            ));
        }

        let id = Ulid::new().to_string();
        if database::get_collection("channels")
            .insert_one(
                doc! {
                    "_id": &id,
                    "nonce": &nonce,
                    "type": 2,
                    "guild": &target.id,
                    "name": &name,
                    "description": &description,
                },
                None,
            )
            .await
            .is_ok()
        {
            if database::get_collection("guilds")
                .update_one(
                    doc! {
                        "_id": &target.id
                    },
                    doc! {
                        "$addToSet": {
                            "channels": &id
                        }
                    },
                    None,
                )
                .await
                .is_ok()
            {
                /*notifications::send_message_threaded(
                    None,
                    target.id.clone(),
                    Notification::guild_channel_create(ChannelCreate {
                        id: target.id.clone(),
                        channel: id.clone(),
                        name: name.clone(),
                        description: description.clone(),
                    }), FIXME
                );*/

                Some(Response::Success(json!({ "id": &id })))
            } else {
                Some(Response::InternalServerError(
                    json!({ "error": "Couldn't save channel list." }),
                ))
            }
        } else {
            Some(Response::InternalServerError(
                json!({ "error": "Couldn't create channel." }),
            ))
        }
    } else {
        Some(Response::BadRequest(
            json!({ "error": "Failed to check if channel was made." }),
        ))
    }
}

#[derive(Serialize, Deserialize)]
pub struct InviteOptions {
    // ? TODO: add options
}

/// create a new invite
#[post("/<target>/channels/<channel>/invite", data = "<_options>")]
pub async fn create_invite(
    user: User,
    target: Guild,
    channel: Channel,
    _options: Json<InviteOptions>,
) -> Option<Response> {
    let (permissions, _) = with_permissions!(user, target);

    if !permissions.get_create_invite() {
        return Some(Response::LackingPermission(Permission::CreateInvite));
    }

    let code = gen_token(7);
    if database::get_collection("guilds")
        .update_one(
            doc! { "_id": target.id },
            doc! {
                "$push": {
                    "invites": {
                        "code": &code,
                        "creator": user.id,
                        "channel": channel.id,
                    }
                }
            },
            None,
        )
        .await
        .is_ok()
    {
        Some(Response::Success(json!({ "code": code })))
    } else {
        Some(Response::BadRequest(
            json!({ "error": "Failed to create invite." }),
        ))
    }
}

/// remove an invite
#[delete("/<target>/invites/<code>")]
pub async fn remove_invite(user: User, target: Guild, code: String) -> Option<Response> {
    let (permissions, _) = with_permissions!(user, target);

    if let Some((guild_id, _, invite)) = get_invite(&code, None).await {
        if invite.creator != user.id && !permissions.get_manage_server() {
            return Some(Response::LackingPermission(Permission::ManageServer));
        }

        if database::get_collection("guilds")
            .update_one(
                doc! {
                    "_id": &guild_id,
                },
                doc! {
                    "$pull": {
                        "invites": {
                            "code": &code
                        }
                    }
                },
                None,
            )
            .await
            .is_ok()
        {
            Some(Response::Result(super::Status::Ok))
        } else {
            Some(Response::BadRequest(
                json!({ "error": "Failed to delete invite." }),
            ))
        }
    } else {
        Some(Response::NotFound(
            json!({ "error": "Failed to fetch invite or code is invalid." }),
        ))
    }
}

/// fetch all guild invites
#[get("/<target>/invites")]
pub async fn fetch_invites(user: User, target: Guild) -> Option<Response> {
    let (permissions, _) = with_permissions!(user, target);

    if !permissions.get_manage_server() {
        return Some(Response::LackingPermission(Permission::ManageServer));
    }

    Some(Response::Success(json!(target.invites)))
}

/// view an invite before joining
#[get("/join/<code>", rank = 1)]
pub async fn fetch_invite(user: User, code: String) -> Response {
    if let Some((guild_id, name, invite)) = get_invite(&code, user.id).await {
        match fetch_channel(&invite.channel).await {
            Ok(result) => {
                if let Some(channel) = result {
                    Response::Success(json!({
                        "guild": {
                            "id": guild_id,
                            "name": name,
                        },
                        "channel": {
                            "id": channel.id,
                            "name": channel.name,
                        }
                    }))
                } else {
                    Response::NotFound(json!({ "error": "Channel does not exist." }))
                }
            }
            Err(err) => Response::InternalServerError(json!({ "error": err })),
        }
    } else {
        Response::NotFound(json!({ "error": "Failed to fetch invite or code is invalid." }))
    }
}

/// join a guild using an invite
#[post("/join/<code>", rank = 1)]
pub async fn use_invite(user: User, code: String) -> Response {
    if let Some((guild_id, _, invite)) = get_invite(&code, Some(user.id.clone())).await {
        if let Ok(result) = database::get_collection("members").find_one(
            doc! {
                "_id.guild": &guild_id,
                "_id.user": &user.id
            },
            FindOneOptions::builder()
                .projection(doc! { "_id": 1 })
                .build(),
        )
        .await {
            if result.is_none() {
                if database::get_collection("members")
                    .insert_one(
                        doc! {
                            "_id": {
                                "guild": &guild_id,
                                "user": &user.id
                            }
                        },
                        None,
                    )
                    .await
                    .is_ok()
                {
                    /*notifications::send_message_threaded(
                        None,
                        guild_id.clone(),
                        Notification::guild_user_join(UserJoin {
                            id: guild_id.clone(),
                            user: user.id.clone(),
                        }), FIXME
                    );*/

                    Response::Success(json!({
                        "guild": &guild_id,
                        "channel": &invite.channel,
                    }))
                } else {
                    Response::InternalServerError(
                        json!({ "error": "Failed to add you to the guild." }),
                    )
                }
            } else {
                Response::BadRequest(json!({ "error": "Already in the guild." }))
            }
        } else {
            Response::InternalServerError(
                json!({ "error": "Failed to check if you're in the guild." }),
            )
        }
    } else {
        Response::NotFound(json!({ "error": "Failed to fetch invite or code is invalid." }))
    }
}

#[derive(Serialize, Deserialize)]
pub struct CreateGuild {
    name: String,
    description: Option<String>,
    nonce: String,
}

/// create a new guild
#[post("/create", data = "<info>")]
pub async fn create_guild(user: User, info: Json<CreateGuild>) -> Response {
    if !user.email_verification.verified {
        return Response::Unauthorized(json!({ "error": "Email not verified!" }));
    }

    let name: String = info.name.chars().take(32).collect();
    let description: String = info
        .description
        .clone()
        .unwrap_or("No description.".to_string())
        .chars()
        .take(255)
        .collect();
    let nonce: String = info.nonce.chars().take(32).collect();

    let channels = database::get_collection("channels");
    let col = database::get_collection("guilds");
    if col
        .find_one(doc! { "nonce": nonce.clone() }, None)
        .await
        .unwrap()
        .is_some()
    {
        return Response::BadRequest(json!({ "error": "Guild already created!" }));
    }

    let id = Ulid::new().to_string();
    let channel_id = Ulid::new().to_string();
    if channels
        .insert_one(
            doc! {
                "_id": channel_id.clone(),
                "type": ChannelType::GUILDCHANNEL as u32,
                "name": "general",
                "description": "",
                "guild": id.clone(),
            },
            None,
        )
        .await
        .is_err()
    {
        return Response::InternalServerError(
            json!({ "error": "Failed to create guild channel." }),
        );
    }

    if database::get_collection("members")
        .insert_one(
            doc! {
                "_id": {
                    "guild": &id,
                    "user": &user.id
                }
            },
            None,
        )
        .await
        .is_err()
    {
        return Response::InternalServerError(
            json!({ "error": "Failed to add you to members list." }),
        );
    }

    if col
        .insert_one(
            doc! {
                "_id": &id,
                "nonce": nonce,
                "name": name,
                "description": description,
                "owner": &user.id,
                "channels": [ channel_id.clone() ],
                "invites": [],
                "bans": [],
                "default_permissions": 51,
            },
            None,
        )
        .await
        .is_ok()
    {
        Response::Success(json!({ "id": id }))
    } else {
        channels
            .delete_one(doc! { "_id": channel_id }, None)
            .await
            .expect("Failed to delete the channel we just made.");

        Response::InternalServerError(json!({ "error": "Failed to create guild." }))
    }
}

/// fetch a guild's member
#[get("/<target>/members")]
pub async fn fetch_members(user: User, target: Guild) -> Option<Response> {
    with_permissions!(user, target);

    if let Ok(mut result) =
        database::get_collection("members").find(doc! { "_id.guild": target.id }, None)
        .await
    {
        let mut users = vec![];

        while let Some(item) = result.next().await {
            if let Ok(doc) = item {
                users.push(json!({
                    "id": doc.get_document("_id").unwrap().get_str("user").unwrap(),
                    "nickname": doc.get_str("nickname").ok(),
                }));
            }
        }

        Some(Response::Success(json!(users)))
    } else {
        Some(Response::InternalServerError(
            json!({ "error": "Failed to fetch members." }),
        ))
    }
}

/// fetch a guild member
#[get("/<target>/members/<other>")]
pub async fn fetch_member(user: User, target: Guild, other: String) -> Option<Response> {
    with_permissions!(user, target);

    if let Ok(result) = get_member(MemberKey(target.id, other)).await {
        if let Some(member) = result {
            Some(Response::Success(json!({
                "id": member.id.user,
                "nickname": member.nickname,
            })))
        } else {
            Some(Response::NotFound(
                json!({ "error": "Member does not exist!" }),
            ))
        }
    } else {
        Some(Response::InternalServerError(
            json!({ "error": "Failed to fetch member." }),
        ))
    }
}

/// kick a guild member
#[delete("/<target>/members/<other>")]
pub async fn kick_member(user: User, target: Guild, other: String) -> Option<Response> {
    let (permissions, _) = with_permissions!(user, target);

    if user.id == other {
        return Some(Response::BadRequest(
            json!({ "error": "Cannot kick yourself." }),
        ));
    }

    if !permissions.get_kick_members() {
        return Some(Response::LackingPermission(Permission::KickMembers));
    }

    if let Ok(result) = get_member(MemberKey(target.id.clone(), other.clone())).await {
        if result.is_none() {
            return Some(Response::BadRequest(
                json!({ "error": "User not part of guild." }),
            ));
        }
    } else {
        return Some(Response::InternalServerError(
            json!({ "error": "Failed to fetch member." }),
        ));
    }

    if database::get_collection("members")
        .delete_one(
            doc! {
                "_id.guild": &target.id,
                "_id.user": &other,
            },
            None,
        )
        .await
        .is_ok()
    {
        /*notifications::send_message_threaded(
            None,
            target.id.clone(),
            Notification::guild_user_leave(UserLeave {
                id: target.id.clone(),
                user: other.clone(),
                banned: false,
            }), FIXME
        );*/

        Some(Response::Result(super::Status::Ok))
    } else {
        Some(Response::InternalServerError(
            json!({ "error": "Failed to kick member." }),
        ))
    }
}

#[derive(Serialize, Deserialize, FromForm)]
pub struct BanOptions {
    reason: Option<String>,
}

/// ban a guild member
#[put("/<target>/members/<other>/ban?<options..>")]
pub async fn ban_member(
    user: User,
    target: Guild,
    other: String,
    options: Form<BanOptions>,
) -> Option<Response> {
    let (permissions, _) = with_permissions!(user, target);
    let reason: String = options
        .reason
        .clone()
        .unwrap_or("No reason specified.".to_string())
        .chars()
        .take(64)
        .collect();

    if user.id == other {
        return Some(Response::BadRequest(
            json!({ "error": "Cannot ban yourself." }),
        ));
    }

    if !permissions.get_ban_members() {
        return Some(Response::LackingPermission(Permission::BanMembers));
    }

    if let Ok(result) = get_member(MemberKey(target.id.clone(), other.clone())).await {
        if result.is_none() {
            return Some(Response::BadRequest(
                json!({ "error": "User not part of guild." }),
            ));
        }
    } else {
        return Some(Response::InternalServerError(
            json!({ "error": "Failed to fetch member." }),
        ));
    }

    if database::get_collection("guilds")
        .update_one(
            doc! { "_id": &target.id },
            doc! {
                "$push": {
                    "bans": {
                        "id": &other,
                        "reason": reason,
                    }
                }
            },
            None,
        )
        .await
        .is_err()
    {
        return Some(Response::BadRequest(
            json!({ "error": "Failed to add ban to guild." }),
        ));
    }

    if database::get_collection("members")
        .delete_one(
            doc! {
                "_id.guild": &target.id,
                "_id.user": &other,
            },
            None,
        )
        .await
        .is_ok()
    {
        /*notifications::send_message_threaded(
            None,
            target.id.clone(),
            Notification::guild_user_leave(UserLeave {
                id: target.id.clone(),
                user: other.clone(),
                banned: true,
            }), FIXME
        );*/

        Some(Response::Result(super::Status::Ok))
    } else {
        Some(Response::InternalServerError(
            json!({ "error": "Failed to kick member after adding to ban list." }),
        ))
    }
}

/// unban a guild member
#[delete("/<target>/members/<other>/ban")]
pub async fn unban_member(user: User, target: Guild, other: String) -> Option<Response> {
    let (permissions, _) = with_permissions!(user, target);

    if user.id == other {
        return Some(Response::BadRequest(
            json!({ "error": "Cannot unban yourself (not checking if you're banned)." }),
        ));
    }

    if !permissions.get_ban_members() {
        return Some(Response::LackingPermission(Permission::BanMembers));
    }

    if target.bans.iter().any(|v| v.id == other) {
        return Some(Response::BadRequest(json!({ "error": "User not banned." })));
    }

    if database::get_collection("guilds")
        .update_one(
            doc! {
                "_id": &target.id
            },
            doc! {
                "$pull": {
                    "bans": {
                        "$elemMatch": {
                            "id": &other
                        }
                    }
                }
            },
            None,
        )
        .await
        .is_ok()
    {
        Some(Response::Result(super::Status::Ok))
    } else {
        Some(Response::BadRequest(
            json!({ "error": "Failed to remove ban." }),
        ))
    }
}
