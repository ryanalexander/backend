use revolt_quark::{models::User, perms, Db, EmptyResponse, Ref, Result};

#[put("/<target>/ack")]
pub async fn req(db: &Db, user: User, target: Ref) -> Result<EmptyResponse> {
    let server = target.as_server(db).await?;
    perms(&user).server(&server).calc(db).await?;

    db.acknowledge_channels(&user.id, &server.channels)
        .await
        .map(|_| EmptyResponse)
}
