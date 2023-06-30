use crate::types::{CCIPReadHandler, RPCCall, RPCResponse};
use crate::CCIPReadMiddlewareError;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use ethers_core::abi::{Abi, Function};
use ethers_core::utils::hex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::debug;

type Handlers = HashMap<[u8; 4], (Function, Arc<dyn CCIPReadHandler + Sync + Send>)>;

struct AppState {
    handlers: Handlers,
}

/// CCIP-Read Server.
#[derive(Clone)]
pub struct Server {
    ip_address: IpAddr,
    port: u16,
    handlers: Handlers,
}

#[derive(Deserialize)]
pub struct CCIPReadMiddlewareRequest {
    sender: String,
    calldata: String,
}

impl Server {
    /// Create a new server
    ///
    /// # Arguments
    /// * `ip_address` the IP address to bind to
    /// * `port` the port the server should bind to
    pub fn new(ip_address: IpAddr, port: u16) -> Self {
        Server {
            ip_address,
            port,
            handlers: HashMap::new(),
        }
    }

    /// Add callbacks for CCIP-Read server requests
    ///
    /// # Arguments
    /// * `abi` the parsed ABI of the contract to decode data for
    /// * `handlers` the callbacks
    pub fn add(
        &mut self,
        abi: Abi,
        name: &str,
        callback: Arc<dyn CCIPReadHandler + Sync + Send>,
    ) -> Result<(), CCIPReadMiddlewareError> {
        let function = abi.function(name)?.clone();
        debug!(
            "Added function with short sig: {:?}",
            function.short_signature()
        );
        self.handlers
            .insert(function.short_signature(), (function, callback));
        Ok(())
    }

    /// Starts a new CCIP-Read server.
    ///
    /// # Arguments
    /// * `router` an optional Axum router to merge with the CCIP-Read one provided by the library
    pub async fn start(&self, router: Option<Router>) -> Result<(), CCIPReadMiddlewareError> {
        let ccip_router = self.router();
        let app: Router = if let Some(router) = router {
            router.merge(ccip_router)
        } else {
            ccip_router
        };

        let bound_interface: SocketAddr = SocketAddr::new(self.ip_address, self.port);
        let _ = axum::Server::bind(&bound_interface)
            .serve(app.into_make_service())
            .await;
        Ok(())
    }

    fn router(&self) -> Router {
        let shared_state = Arc::new(AppState {
            handlers: self.handlers.clone(),
        });
        Router::new()
            .route("/gateway/:sender/:calldata", get(gateway_get))
            .route("/gateway", post(gateway_post))
            .with_state(shared_state)
            .layer(TraceLayer::new_for_http())
    }
}

async fn gateway_get(
    Path((sender, calldata)): Path<(String, String)>,
    State(app_state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    let calldata = String::from(calldata.strip_suffix(".json").unwrap_or(calldata.as_str()));
    debug!("Should handle sender={:?} calldata={}", sender, calldata);

    if let Ok(calldata) = ethers_core::types::Bytes::from_str(&calldata.as_str()[2..]) {
        let response = call(
            RPCCall {
                to: sender.clone(),
                data: calldata,
            },
            app_state.handlers.clone(),
        )
        .await
        .unwrap();

        let body = response.body;
        Ok((StatusCode::OK, Json(body)))
    } else {
        let error_message: Value = json!({
            "message": "Unexpected error",
        });
        Ok((StatusCode::INTERNAL_SERVER_ERROR, Json(error_message)))
    }
}

async fn gateway_post(
    State(app_state): State<Arc<AppState>>,
    Json(data): Json<CCIPReadMiddlewareRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let sender = data.sender;
    let calldata = String::from(
        data.calldata
            .strip_suffix(".json")
            .unwrap_or(data.calldata.as_str()),
    );
    debug!("Should handle sender={:?} calldata={}", sender, calldata);

    if let Ok(calldata) = ethers_core::types::Bytes::from_str(&calldata.as_str()[2..]) {
        let response = call(
            RPCCall {
                to: sender.clone(),
                data: calldata,
            },
            app_state.handlers.clone(),
        )
        .await
        .unwrap();

        let body = response.body;
        Ok((StatusCode::OK, Json(body)))
    } else {
        let error_message: Value = json!({
            "message": "Unexpected error",
        });
        Ok((StatusCode::INTERNAL_SERVER_ERROR, Json(error_message)))
    }
}

#[tracing::instrument(
    name = "ccip_server"
    skip_all
)]
async fn call(call: RPCCall, handlers: Handlers) -> Result<RPCResponse, CCIPReadMiddlewareError> {
    debug!("Received call with {:?}", call);
    let selector = &call.data[0..4];

    // find a function handler for this selector
    let handler = if let Some(handler) = handlers.get(selector) {
        handler
    } else {
        return Ok(RPCResponse {
            status: 404,
            body: json!({
                "message": format!("No implementation for function with selector 0x{}", hex::encode(selector)),
            }),
        });
    };

    // decode function arguments
    let args = handler.0.decode_input(&call.data[4..])?;

    let callback = handler.1.clone();
    if let Ok(tokens) = callback
        .call(
            args,
            RPCCall {
                to: call.to,
                data: call.data,
            },
        )
        .await
    {
        let encoded_data = ethers_core::abi::encode(&tokens);
        let encoded_data = format!("0x{}", hex::encode(encoded_data));
        debug!("Final encoded data: {}", encoded_data);

        Ok(RPCResponse {
            status: 200,
            body: json!({
                "data": encoded_data,
            }),
        })
    } else {
        Ok(RPCResponse {
            status: 500,
            body: json!({
                "message": "Unexpected error",
            }),
        })
    }
}

// Sample ENS offchain resolver request:
// http://localhost:8080/gateway/0x8464135c8f25da09e49bc8782676a84730c318bc/0x9061b92300000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000000a047465737403657468000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000243b3b57deeb4f647bea6caa36333c816d7b46fdcb05f9466ecacc140ea8c66faf15b3d9f100000000000000000000000000000000000000000000000000000000.json
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use ethers::abi::AbiParser;
    use ethers::contract::BaseContract;
    use serde_json::{json, Value};
    use tower::ServiceExt; // for `oneshot` and `ready`

    #[test]
    fn it_parse_offchain_resolver_abi() {
        let abi = AbiParser::default().parse_str(r#"[
            function resolve(bytes memory name, bytes memory data) external view returns(bytes memory)
        ]"#).unwrap();
        let contract = BaseContract::from(abi);
        println!("{:?}", contract.methods);
    }

    #[tokio::test]
    async fn test_gateway_get_on_unknown_selector() {
        let server = Server::new(IpAddr::V4("127.0.0.1".parse().unwrap()), 8080);
        let router = server.router();

        let response = router
            .oneshot(Request::builder().uri("/gateway/0x8464135c8f25da09e49bc8782676a84730c318bc/0x9061b92300000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000000a0474657374036574680000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008459d1d43ceb4f647bea6caa36333c816d7b46fdcb05f9466ecacc140ea8c66faf15b3d9f100000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000005656d61696c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.json").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body,
            json!({ "message": "No implementation for function with selector 0x9061b923"})
        );
    }
}
