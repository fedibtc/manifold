use std::error::Error;

use fedi_decentralized_push_gateway::{
    AppState, Database, DeliveryOutboxRepository, OutboxAdminRow, OutboxDeadLetterSelector,
    PushGatewayArgs, PushGatewayCommand, PushGatewayOutboxAction, PushGatewayOutboxCommand,
    operator_app, public_app,
};
use tokio::net::TcpListener;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    match PushGatewayCommand::parse() {
        PushGatewayCommand::Serve(args) => serve(args).await,
        PushGatewayCommand::Outbox(command) => run_outbox_command(command).await,
    }
}

async fn serve(args: PushGatewayArgs) -> Result<(), Box<dyn Error>> {
    let bind_addr = args.bind();
    let config = args.into_config()?;
    let operator_bind_addr = config.operator_bind();
    let state = AppState::connect(config).await?;
    let worker = state.start_delivery_worker();
    let listener = TcpListener::bind(bind_addr).await?;
    let operator_listener = match operator_bind_addr {
        Some(operator_bind_addr) => Some(TcpListener::bind(operator_bind_addr).await?),
        None => None,
    };

    let public_server = axum::serve(
        listener,
        public_app(state.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal());

    if let Some(operator_listener) = operator_listener {
        let operator_server = axum::serve(
            operator_listener,
            operator_app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal());
        tokio::try_join!(public_server, operator_server)?;
    } else {
        public_server.await?;
    }
    worker.shutdown().await;
    Ok(())
}

async fn run_outbox_command(command: PushGatewayOutboxCommand) -> Result<(), Box<dyn Error>> {
    let (database_url, action) = command.into_parts();
    let database = Database::connect_existing(&database_url).await?;
    let outbox = DeliveryOutboxRepository::new(database.pool().clone(), database.backend());
    match action {
        PushGatewayOutboxAction::ListDeadLetter { limit, json } => {
            let rows = outbox.list_dead_letter_rows(limit).await?;
            print_rows(&rows, json)?;
            eprintln!("dead_letter_rows={}", rows.len());
        }
        PushGatewayOutboxAction::DeadLetterReasons { json } => {
            let reasons = outbox.dead_letter_reason_counts().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&reasons)?);
            } else {
                println!("count\treason");
                for reason in reasons {
                    println!("{}\t{}", reason.count, table_value(&reason.reason));
                }
            }
        }
        PushGatewayOutboxAction::ReplayDeadLetter(args) => {
            let selector = selector_from_args(&args)?;
            let rows = outbox.select_dead_letter_rows(&selector).await?;
            print_rows(&rows, args.json)?;
            ensure_confirmed_mutation(&args, "replay")?;
            let changed = outbox
                .replay_dead_letter_rows(&selector, args.dry_run)
                .await?;
            eprintln!(
                "{}={changed}",
                if args.dry_run {
                    "would_replay_dead_letter_rows"
                } else {
                    "replayed_dead_letter_rows"
                }
            );
        }
        PushGatewayOutboxAction::DeleteDeadLetter(args) => {
            let selector = selector_from_args(&args)?;
            let rows = outbox.select_dead_letter_rows(&selector).await?;
            print_rows(&rows, args.json)?;
            ensure_confirmed_mutation(&args, "permanently delete")?;
            let changed = outbox
                .delete_dead_letter_rows(&selector, args.dry_run)
                .await?;
            eprintln!(
                "{}={changed}",
                if args.dry_run {
                    "would_delete_dead_letter_rows"
                } else {
                    "deleted_dead_letter_rows"
                }
            );
        }
    }
    Ok(())
}

fn selector_from_args(
    args: &fedi_decentralized_push_gateway::DeadLetterMutationArgs,
) -> Result<OutboxDeadLetterSelector, Box<dyn Error>> {
    if args.outbox_ids.is_empty() && args.limit.is_none() {
        return Err(
            "dead-letter mutation requires at least one --outbox-id or a bounded --limit".into(),
        );
    }
    Ok(OutboxDeadLetterSelector {
        outbox_ids: args.outbox_ids.clone(),
        limit: args.limit,
        reason: args.reason.clone(),
    })
}

fn ensure_confirmed_mutation(
    args: &fedi_decentralized_push_gateway::DeadLetterMutationArgs,
    operation: &'static str,
) -> Result<(), Box<dyn Error>> {
    if !args.dry_run && !args.yes {
        return Err(
            format!("refusing to {operation} without --yes (use --dry-run to preview)").into(),
        );
    }
    Ok(())
}

fn print_rows(rows: &[OutboxAdminRow], json: bool) -> Result<(), Box<dyn Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(rows)?);
        return Ok(());
    }
    println!(
        "outbox_id\tevent_id\trecipient_id\tinstallation_id\tplatform\tattempts\tlast_error\tcreated_at\tupdated_at"
    );
    for row in rows {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            table_value(&row.outbox_id),
            table_value(&row.event_id),
            table_value(&row.recipient_id),
            table_value(&row.installation_id),
            table_value(row.platform.as_deref().unwrap_or("-")),
            row.attempts,
            table_value(row.last_error.as_deref().unwrap_or("unknown")),
            row.created_at,
            row.updated_at
        );
    }
    Ok(())
}

fn table_value(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<invalid>\"".to_owned())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
