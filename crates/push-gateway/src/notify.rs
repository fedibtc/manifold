use axum::Json;

use crate::{
    ApiError, AuthenticatedJson, HookNotificationRequest, Notification, NotificationHookResponse,
    validation::{
        MAX_BODY_LEN, MAX_ID_LEN, MAX_TITLE_LEN, validate_data, validate_optional_string,
        validate_required_string,
    },
};

pub(crate) async fn notification_hook(
    AuthenticatedJson {
        recipient_id,
        payload: request,
    }: AuthenticatedJson<HookNotificationRequest>,
) -> Result<Json<NotificationHookResponse>, ApiError> {
    validate_notification_hook_request(&request)?;

    let notification = Notification::from_authenticated(recipient_id, request);

    Ok(Json(NotificationHookResponse::accepted(notification)))
}

fn validate_notification_hook_request(request: &HookNotificationRequest) -> Result<(), ApiError> {
    validate_required_string("notification_id", &request.notification_id.0, MAX_ID_LEN)?;
    validate_required_string("kind", &request.kind.0, MAX_ID_LEN)?;
    validate_optional_string("title", request.title.as_deref(), MAX_TITLE_LEN)?;
    validate_optional_string("body", request.body.as_deref(), MAX_BODY_LEN)?;
    validate_data(&request.data)?;
    Ok(())
}
