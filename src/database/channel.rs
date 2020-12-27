use super::get_collection;

use lru::LruCache;
use mongodb::bson::{doc, from_bson, Bson};
use rocket::http::RawStr;
use rocket::request::FromParam;
use rocket_contrib::json::JsonValue;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use rocket::futures::StreamExt;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LastMessage {
    // message id
    id: String,
    // author's id
    user_id: String,
    // truncated content with author's name prepended (for GDM / GUILD)
    short_content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Channel {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: u8,

    // DM: whether the DM is active
    pub active: Option<bool>,
    // DM + GDM: last message in channel
    pub last_message: Option<LastMessage>,
    // DM + GDM: recipients for channel
    pub recipients: Option<Vec<String>>,
    // GDM: owner of group
    pub owner: Option<String>,
    // GUILD: channel parent
    pub guild: Option<String>,
    // GUILD + GDM: channel name
    pub name: Option<String>,
    // GUILD + GDM: channel description
    pub description: Option<String>,
}

impl Channel {
    pub fn serialise(self) -> JsonValue {
        match self.channel_type {
            0 => json!({
                "id": self.id,
                "type": self.channel_type,
                "last_message": self.last_message,
                "recipients": self.recipients,
            }),
            1 => json!({
                "id": self.id,
                "type": self.channel_type,
                "last_message": self.last_message,
                "recipients": self.recipients,
                "name": self.name,
                "owner": self.owner,
                "description": self.description,
            }),
            2 => json!({
                "id": self.id,
                "type": self.channel_type,
                "guild": self.guild,
                "name": self.name,
                "description": self.description,
            }),
            _ => unreachable!(),
        }
    }
}

lazy_static! {
    static ref CACHE: Arc<Mutex<LruCache<String, Channel>>> =
        Arc::new(Mutex::new(LruCache::new(4_000_000)));
}

pub async fn fetch_channel(id: &str) -> Result<Option<Channel>, String> {
    {
        if let Ok(mut cache) = CACHE.lock() {
            let existing = cache.get(&id.to_string());

            if let Some(channel) = existing {
                return Ok(Some((*channel).clone()));
            }
        } else {
            return Err("Failed to lock cache.".to_string());
        }
    }

    let col = get_collection("channels");
    if let Ok(result) = col.find_one(doc! { "_id": id }, None).await {
        if let Some(doc) = result {
            if let Ok(channel) = from_bson(Bson::Document(doc)) as Result<Channel, _> {
                let mut cache = CACHE.lock().unwrap();
                cache.put(id.to_string(), channel.clone());

                Ok(Some(channel))
            } else {
                Err("Failed to deserialize channel!".to_string())
            }
        } else {
            Ok(None)
        }
    } else {
        Err("Failed to fetch channel from database.".to_string())
    }
}

pub async fn fetch_channels(ids: &Vec<String>) -> Result<Vec<Channel>, String> {
    let mut missing = vec![];
    let mut channels = vec![];

    {
        if let Ok(mut cache) = CACHE.lock() {
            for id in ids {
                let existing = cache.get(id);

                if let Some(channel) = existing {
                    channels.push((*channel).clone());
                } else {
                    missing.push(id);
                }
            }
        } else {
            return Err("Failed to lock cache.".to_string());
        }
    }

    if missing.len() == 0 {
        return Ok(channels);
    }

    let col = get_collection("channels");
    if let Ok(mut result) = col.find(doc! { "_id": { "$in": missing } }, None).await {
        while let Some(item) = result.next().await {
            let mut cache = CACHE.lock().unwrap();
            if let Ok(doc) = item {
                if let Ok(channel) = from_bson(Bson::Document(doc)) as Result<Channel, _> {
                    cache.put(channel.id.clone(), channel.clone());
                    channels.push(channel);
                } else {
                    return Err("Failed to deserialize channel!".to_string());
                }
            } else {
                return Err("Failed to fetch channel.".to_string());
            }
        }

        Ok(channels)
    } else {
        Err("Failed to fetch channel from database.".to_string())
    }
}

impl<'r> FromParam<'r> for Channel {
    type Error = &'r RawStr;

    fn from_param(param: &'r RawStr) -> Result<Self, Self::Error> {
        Err(param)
        /*if let Ok(result) = fetch_channel(param).await {
            if let Some(channel) = result {
                Ok(channel)
            } else {
                Err(param)
            }
        } else {
            Err(param)
        }*/
    }
}

/*use crate::notifications::events::Notification;

pub fn process_event(event: &Notification) {
    match event {
        Notification::group_user_join(ev) => {
            let mut cache = CACHE.lock().unwrap();
            if let Some(channel) = cache.peek_mut(&ev.id) {
                channel.recipients.as_mut().unwrap().push(ev.user.clone());
            }
        }
        Notification::group_user_leave(ev) => {
            let mut cache = CACHE.lock().unwrap();
            if let Some(channel) = cache.peek_mut(&ev.id) {
                let recipients = channel.recipients.as_mut().unwrap();
                if let Some(pos) = recipients.iter().position(|x| *x == ev.user) {
                    recipients.remove(pos);
                }
            }
        }
        Notification::guild_channel_create(ev) => {
            let mut cache = CACHE.lock().unwrap();
            cache.put(
                ev.id.clone(),
                Channel {
                    id: ev.channel.clone(),
                    channel_type: 2,
                    active: None,
                    last_message: None,
                    recipients: None,
                    owner: None,
                    guild: Some(ev.id.clone()),
                    name: Some(ev.name.clone()),
                    description: Some(ev.description.clone()),
                },
            );
        }
        Notification::guild_channel_delete(ev) => {
            let mut cache = CACHE.lock().unwrap();
            cache.pop(&ev.channel);
        }
        _ => {}
    }
}*/
