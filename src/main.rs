use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use axum_extra::TypedHeader;
use tracing::info;

use std::ops::ControlFlow;
use std::time::Duration;
use std::{net::SocketAddr, path::PathBuf};
use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer},
};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

//allows to extract the IP of connecting user
use axum::extract::connect_info::ConnectInfo;

//allows to split the websocket stream into separate TX and RX branches
use futures::{sink::SinkExt, stream::StreamExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "example_websockets=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");

    let app = Router::new()
        // 这里的fallback_service是用来处理没有匹配路由时使用的备用服务
        .fallback_service(ServeDir::new(asset_dir).append_index_html_on_directories(true))
        .route("/ws", get(ws_handler)) // 使用get
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();

    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("Unknown browser")
    };
    println!("`{user_agent}` at {addr} connected.");
    // 每个ws链接都会生成一个handle_socket(socket, addr)线程
    ws.on_upgrade(move |socket| handle_socket(socket, addr))
}

async fn handle_socket(mut socket: WebSocket, who: SocketAddr) {
    // 这里首先给链接过来的ws发送一个数组`vec![1, 2, 3]`
    if socket.send(Message::Ping(vec![1, 2, 3])).await.is_ok() {
        print!("Pinged {who}...");
    } else {
        println!("Could not send ping {who}");
        return;
    }

    // 接收ws发来的消息
    if let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            info!("----> go deal msg: {:?}", msg); // 其实就是client发来的Hello World!
                                                   // 去具体处理发来的请求
            if process_message(msg, who).is_break() {
                info!("client {who} abruptly disconnected, because of flow control is breaked");
                return;
            }
        }
    }

    // 向客户端发送两个消息"Hi 1 times!" 和 "Hi 2 times!"
    for i in 1..3 {
        if socket
            .send(Message::Text(format!("Hi {i} times!")))
            .await
            .is_err()
        {
            println!("2. client {who} abruptly disconnected");
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 调用split()将socket的接收和发送分开
    let (mut sender, mut receiver) = socket.split();

    // 起新线程发送20条消息
    let mut send_task = tokio::spawn(async move {
        let n_msg = 20;
        for i in 0..n_msg {
            if sender
                .send(Message::Text(format!("Server message {i} ...")))
                .await
                .is_err()
            {
                return i;
            }

            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        n_msg
    });

    // 起线程来接收处理客户发来的消息,并计数
    let mut recv_task = tokio::spawn(async move {
        let mut cnt = 0;
        while let Some(Ok(msg)) = receiver.next().await {
            cnt += 1;
            if process_message(msg, who).is_break() {
                break;
            }
        }
        cnt
    });

    //
    tokio::select! {
        rv_a = (&mut send_task) => {
            match rv_a {
                Ok(a) => println!("total {a} message sent to {who}"),
                Err(a) => println!("Error sending message {a:?}")
            }
            //recv_task.abort(); // 这里如果注释要是为了让客户端的消息发完，客户端发30条，服务端20
            //条，如果关了，就接收不到客户端后来发送的消息了
        },
        rv_b =(&mut recv_task) => {
            match rv_b {
                Ok(b) => println!("Received {b} messages"),
                Err(b) => println!("Error receiving message {b:?}")
            }
            send_task.abort();
        }
    }

    println!("3. WebSocket context {who} destroyed");
}

// 对不同的类型的消息分别处理
fn process_message(msg: Message, who: SocketAddr) -> ControlFlow<(), ()> {
    match msg {
        Message::Text(t) => {
            println!(">> {who} sent str: {t:?}");
        }
        Message::Binary(d) => {
            println!(">>> {} sent {} bytes: {:?}", who, d.len(), d);
        }
        Message::Close(c) => {
            if let Some(cf) = c {
                println!(
                    ">>> {} sent close with code {} and reason `{}`",
                    who, cf.code, cf.reason
                );
            } else {
                println!(">>> {who} somehow sent close message without CloseFrame");
            }
            return ControlFlow::Break(()); // 这里手动break会导致is_break()为true
        }

        Message::Pong(v) => {
            println!(">>>>>>> {who} sent pong with {v:?}");
        }
        // You should never need to manually handle Message::Ping, as axum's websocket library
        // will do so for you automagically by replying with Pong and copying the v according to
        // spec. But if you need the contents of the pings you can see them here.
        Message::Ping(v) => {
            println!(">>>>>> {who} sent ping with {v:?}");
            info!("------> {:?}", String::from_utf8(v));
        }
    }
    ControlFlow::Continue(())
}
