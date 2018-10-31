use activitypub::{Activity, Actor, Object, Link};
use array_tool::vec::Uniq;
use futures::{Future, Stream};
use reqwest::async::{Client, Decoder};
use rocket::{
    Outcome, http::Status,
    response::{Response, Responder},
    request::{FromRequest, Request}
};
use serde_json;
use std::{io::{self, Cursor}, mem};
use tokio;

use self::sign::Signable;

pub mod inbox;
pub mod request;
pub mod sign;

pub const CONTEXT_URL: &'static str = "https://www.w3.org/ns/activitystreams";
pub const PUBLIC_VISIBILTY: &'static str = "https://www.w3.org/ns/activitystreams#Public";

pub fn ap_accept_header() -> Vec<&'static str> {
    vec![
        "application/ld+json; profile=\"https://w3.org/ns/activitystreams\"",
        "application/ld+json;profile=\"https://w3.org/ns/activitystreams\"",
        "application/activity+json",
        "application/ld+json"
    ]
}

pub fn context() -> serde_json::Value {
    json!([
        CONTEXT_URL,
        "https://w3id.org/security/v1",
        {
            "manuallyApprovesFollowers": "as:manuallyApprovesFollowers",
            "sensitive": "as:sensitive",
            "movedTo": "as:movedTo",
            "Hashtag": "as:Hashtag",
            "ostatus":"http://ostatus.org#",
            "atomUri":"ostatus:atomUri",
            "inReplyToAtomUri":"ostatus:inReplyToAtomUri",
            "conversation":"ostatus:conversation",
            "toot":"http://joinmastodon.org/ns#",
            "Emoji":"toot:Emoji",
            "focalPoint": {
                "@container":"@list",
                "@id":"toot:focalPoint"
            },
            "featured":"toot:featured"
        }
    ])
}

pub struct ActivityStream<T> (T);

impl<T> ActivityStream<T> {
    pub fn new(t: T) -> ActivityStream<T> {
        ActivityStream(t)
    }
}

impl<'r, O: Object> Responder<'r> for ActivityStream<O> {
    fn respond_to(self, request: &Request) -> Result<Response<'r>, Status> {
        let mut json = serde_json::to_value(&self.0).map_err(|_| Status::InternalServerError)?;
        json["@context"] = context();
        serde_json::to_string(&json).respond_to(request).map(|r| Response::build_from(r)
            .raw_header("Content-Type", "application/activity+json")
            .finalize())
    }
}

#[derive(Clone)]
pub struct ApRequest;
impl<'a, 'r> FromRequest<'a, 'r> for ApRequest {
    type Error = ();

    fn from_request(request: &'a Request<'r>) -> Outcome<Self, (Status, Self::Error), ()> {
        request.headers().get_one("Accept").map(|header| header.split(",").map(|ct| match ct.trim() {
            // bool for Forward: true if found a valid Content-Type for Plume first (HTML), false otherwise
            "application/ld+json; profile=\"https://w3.org/ns/activitystreams\"" |
            "application/ld+json;profile=\"https://w3.org/ns/activitystreams\"" |
            "application/activity+json" |
            "application/ld+json" => Outcome::Success(ApRequest),
            "text/html" => Outcome::Forward(true),
            _ => Outcome::Forward(false)
        }).fold(Outcome::Forward(false), |out, ct| if out.clone().forwarded().unwrap_or(out.is_success()) {
                out
            } else {
                ct
        }).map_forward(|_| ())).unwrap_or(Outcome::Forward(()))
    }
}
pub fn broadcast<S: sign::Signer, A: Activity, T: inbox::WithInbox + Actor>(sender: &S, act: A, to: Vec<T>) {
    let boxes = to.into_iter()
        .filter(|u| !u.is_local())
        .map(|u| u.get_shared_inbox_url().unwrap_or(u.get_inbox_url()))
        .collect::<Vec<String>>()
        .unique();


    let mut act = serde_json::to_value(act).expect("activity_pub::broadcast: serialization error");
    act["@context"] = context();
    let signed = act.sign(sender);

    for inbox in boxes {
        let mut headers = request::headers();
        headers.insert("Digest", request::Digest::digest(signed.to_string()));
        tokio::run(Client::new()
            .post(&inbox[..])
            .headers(headers.clone())
            .header("Signature", request::signature(sender, headers))
            .body(signed.to_string())
            .send()
            .and_then(move |mut res| {
                println!("Successfully sent activity to inbox ({})", inbox);
                let body = mem::replace(res.body_mut(), Decoder::empty());
                body.concat2()
            })
            .map(|body| {
                println!("Response:");
				let mut body = Cursor::new(body);
                io::copy(&mut body, &mut io::stdout())
                	.expect("broadcast: stdout error");
            })
            .map_err(|e| println!("Error while sending to inbox ({:?})", e)))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Id(String);

impl Id {
    pub fn new<T: Into<String>>(id: T) -> Id {
        Id(id.into())
    }
}

impl Into<String> for Id {
    fn into(self) -> String {
        self.0.clone()
    }
}

pub trait IntoId {
    fn into_id(self) -> Id;
}

impl Link for Id {}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Properties)]
#[serde(rename_all = "camelCase")]
pub struct ApSignature {
    #[activitystreams(concrete(PublicKey), functional)]
    pub public_key: Option<serde_json::Value>
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Properties)]
#[serde(rename_all = "camelCase")]
pub struct PublicKey {
    #[activitystreams(concrete(String), functional)]
    pub id: Option<serde_json::Value>,

    #[activitystreams(concrete(String), functional)]
    pub owner: Option<serde_json::Value>,

    #[activitystreams(concrete(String), functional)]
    pub public_key_pem: Option<serde_json::Value>
}

#[derive(Clone, Debug, Default, UnitString)]
#[activitystreams(Hashtag)]
pub struct HashtagType;

#[derive(Clone, Debug, Default, Deserialize, Serialize, Properties)]
#[serde(rename_all = "camelCase")]
pub struct Hashtag {
    #[serde(rename = "type")]
    kind: HashtagType,

    #[activitystreams(concrete(String), functional)]
    pub href: Option<serde_json::Value>,

    #[activitystreams(concrete(String), functional)]
    pub name: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub media_type: String,

    pub content: String,
}

impl Object for Source {}
