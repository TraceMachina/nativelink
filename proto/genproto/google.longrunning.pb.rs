// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.
/// This resource represents a long-running operation that is the result of a
/// network API call.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Operation {
    /// The server-assigned name, which is only unique within the same service that
    /// originally returns it. If you use the default HTTP mapping, the
    /// `name` should be a resource name ending with `operations/{unique_id}`.
    #[prost(string, tag = "1")]
    pub name: std::string::String,
    /// Service-specific metadata associated with the operation.  It typically
    /// contains progress information and common metadata such as create time.
    /// Some services might not provide such metadata.  Any method that returns a
    /// long-running operation should document the metadata type, if any.
    #[prost(message, optional, tag = "2")]
    pub metadata: ::std::option::Option<::prost_types::Any>,
    /// If the value is `false`, it means the operation is still in progress.
    /// If `true`, the operation is completed, and either `error` or `response` is
    /// available.
    #[prost(bool, tag = "3")]
    pub done: bool,
    /// The operation result, which can be either an `error` or a valid `response`.
    /// If `done` == `false`, neither `error` nor `response` is set.
    /// If `done` == `true`, exactly one of `error` or `response` is set.
    #[prost(oneof = "operation::Result", tags = "4, 5")]
    pub result: ::std::option::Option<operation::Result>,
}
pub mod operation {
    /// The operation result, which can be either an `error` or a valid `response`.
    /// If `done` == `false`, neither `error` nor `response` is set.
    /// If `done` == `true`, exactly one of `error` or `response` is set.
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Result {
        /// The error result of the operation in case of failure or cancellation.
        #[prost(message, tag = "4")]
        Error(super::super::rpc::Status),
        /// The normal response of the operation in case of success.  If the original
        /// method returns no data on success, such as `Delete`, the response is
        /// `google.protobuf.Empty`.  If the original method is standard
        /// `Get`/`Create`/`Update`, the response should be the resource.  For other
        /// methods, the response should have the type `XxxResponse`, where `Xxx`
        /// is the original method name.  For example, if the original method name
        /// is `TakeSnapshot()`, the inferred response type is
        /// `TakeSnapshotResponse`.
        #[prost(message, tag = "5")]
        Response(::prost_types::Any),
    }
}
/// The request message for [Operations.GetOperation][google.longrunning.Operations.GetOperation].
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetOperationRequest {
    /// The name of the operation resource.
    #[prost(string, tag = "1")]
    pub name: std::string::String,
}
/// The request message for [Operations.ListOperations][google.longrunning.Operations.ListOperations].
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ListOperationsRequest {
    /// The name of the operation's parent resource.
    #[prost(string, tag = "4")]
    pub name: std::string::String,
    /// The standard list filter.
    #[prost(string, tag = "1")]
    pub filter: std::string::String,
    /// The standard list page size.
    #[prost(int32, tag = "2")]
    pub page_size: i32,
    /// The standard list page token.
    #[prost(string, tag = "3")]
    pub page_token: std::string::String,
}
/// The response message for [Operations.ListOperations][google.longrunning.Operations.ListOperations].
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ListOperationsResponse {
    /// A list of operations that matches the specified filter in the request.
    #[prost(message, repeated, tag = "1")]
    pub operations: ::std::vec::Vec<Operation>,
    /// The standard List next-page token.
    #[prost(string, tag = "2")]
    pub next_page_token: std::string::String,
}
/// The request message for [Operations.CancelOperation][google.longrunning.Operations.CancelOperation].
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct CancelOperationRequest {
    /// The name of the operation resource to be cancelled.
    #[prost(string, tag = "1")]
    pub name: std::string::String,
}
/// The request message for [Operations.DeleteOperation][google.longrunning.Operations.DeleteOperation].
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DeleteOperationRequest {
    /// The name of the operation resource to be deleted.
    #[prost(string, tag = "1")]
    pub name: std::string::String,
}
/// The request message for [Operations.WaitOperation][google.longrunning.Operations.WaitOperation].
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WaitOperationRequest {
    /// The name of the operation resource to wait on.
    #[prost(string, tag = "1")]
    pub name: std::string::String,
    /// The maximum duration to wait before timing out. If left blank, the wait
    /// will be at most the time permitted by the underlying HTTP/RPC protocol.
    /// If RPC context deadline is also specified, the shorter one will be used.
    #[prost(message, optional, tag = "2")]
    pub timeout: ::std::option::Option<::prost_types::Duration>,
}
/// A message representing the message types used by a long-running operation.
///
/// Example:
///
///   rpc LongRunningRecognize(LongRunningRecognizeRequest)
///       returns (google.longrunning.Operation) {
///     option (google.longrunning.operation_info) = {
///       response_type: "LongRunningRecognizeResponse"
///       metadata_type: "LongRunningRecognizeMetadata"
///     };
///   }
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OperationInfo {
    /// Required. The message name of the primary return type for this
    /// long-running operation.
    /// This type will be used to deserialize the LRO's response.
    ///
    /// If the response is in a different package from the rpc, a fully-qualified
    /// message name must be used (e.g. `google.protobuf.Struct`).
    ///
    /// Note: Altering this value constitutes a breaking change.
    #[prost(string, tag = "1")]
    pub response_type: std::string::String,
    /// Required. The message name of the metadata type for this long-running
    /// operation.
    ///
    /// If the response is in a different package from the rpc, a fully-qualified
    /// message name must be used (e.g. `google.protobuf.Struct`).
    ///
    /// Note: Altering this value constitutes a breaking change.
    #[prost(string, tag = "2")]
    pub metadata_type: std::string::String,
}
#[doc = r" Generated client implementations."]
pub mod operations_client {
    #![allow(unused_variables, dead_code, missing_docs)]
    use tonic::codegen::*;
    #[doc = " Manages long-running operations with an API service."]
    #[doc = ""]
    #[doc = " When an API method normally takes long time to complete, it can be designed"]
    #[doc = " to return [Operation][google.longrunning.Operation] to the client, and the client can use this"]
    #[doc = " interface to receive the real response asynchronously by polling the"]
    #[doc = " operation resource, or pass the operation resource to another API (such as"]
    #[doc = " Google Cloud Pub/Sub API) to receive the response.  Any API service that"]
    #[doc = " returns long-running operations should implement the `Operations` interface"]
    #[doc = " so developers can have a consistent client experience."]
    pub struct OperationsClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl OperationsClient<tonic::transport::Channel> {
        #[doc = r" Attempt to create a new client by connecting to a given endpoint."]
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: std::convert::TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }
    impl<T> OperationsClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::BoxBody>,
        T::ResponseBody: Body + HttpBody + Send + 'static,
        T::Error: Into<StdError>,
        <T::ResponseBody as HttpBody>::Error: Into<StdError> + Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }
        pub fn with_interceptor(inner: T, interceptor: impl Into<tonic::Interceptor>) -> Self {
            let inner = tonic::client::Grpc::with_interceptor(inner, interceptor);
            Self { inner }
        }
        #[doc = " Lists operations that match the specified filter in the request. If the"]
        #[doc = " server doesn't support this method, it returns `UNIMPLEMENTED`."]
        #[doc = ""]
        #[doc = " NOTE: the `name` binding allows API services to override the binding"]
        #[doc = " to use different resource name schemes, such as `users/*/operations`. To"]
        #[doc = " override the binding, API services can add a binding such as"]
        #[doc = " `\"/v1/{name=users/*}/operations\"` to their service configuration."]
        #[doc = " For backwards compatibility, the default name includes the operations"]
        #[doc = " collection id, however overriding users must ensure the name binding"]
        #[doc = " is the parent resource, without the operations collection id."]
        pub async fn list_operations(
            &mut self,
            request: impl tonic::IntoRequest<super::ListOperationsRequest>,
        ) -> Result<tonic::Response<super::ListOperationsResponse>, tonic::Status> {
            self.inner.ready().await.map_err(|e| {
                tonic::Status::new(
                    tonic::Code::Unknown,
                    format!("Service was not ready: {}", e.into()),
                )
            })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/google.longrunning.Operations/ListOperations",
            );
            self.inner.unary(request.into_request(), path, codec).await
        }
        #[doc = " Gets the latest state of a long-running operation.  Clients can use this"]
        #[doc = " method to poll the operation result at intervals as recommended by the API"]
        #[doc = " service."]
        pub async fn get_operation(
            &mut self,
            request: impl tonic::IntoRequest<super::GetOperationRequest>,
        ) -> Result<tonic::Response<super::Operation>, tonic::Status> {
            self.inner.ready().await.map_err(|e| {
                tonic::Status::new(
                    tonic::Code::Unknown,
                    format!("Service was not ready: {}", e.into()),
                )
            })?;
            let codec = tonic::codec::ProstCodec::default();
            let path =
                http::uri::PathAndQuery::from_static("/google.longrunning.Operations/GetOperation");
            self.inner.unary(request.into_request(), path, codec).await
        }
        #[doc = " Deletes a long-running operation. This method indicates that the client is"]
        #[doc = " no longer interested in the operation result. It does not cancel the"]
        #[doc = " operation. If the server doesn't support this method, it returns"]
        #[doc = " `google.rpc.Code.UNIMPLEMENTED`."]
        pub async fn delete_operation(
            &mut self,
            request: impl tonic::IntoRequest<super::DeleteOperationRequest>,
        ) -> Result<tonic::Response<()>, tonic::Status> {
            self.inner.ready().await.map_err(|e| {
                tonic::Status::new(
                    tonic::Code::Unknown,
                    format!("Service was not ready: {}", e.into()),
                )
            })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/google.longrunning.Operations/DeleteOperation",
            );
            self.inner.unary(request.into_request(), path, codec).await
        }
        #[doc = " Starts asynchronous cancellation on a long-running operation.  The server"]
        #[doc = " makes a best effort to cancel the operation, but success is not"]
        #[doc = " guaranteed.  If the server doesn't support this method, it returns"]
        #[doc = " `google.rpc.Code.UNIMPLEMENTED`.  Clients can use"]
        #[doc = " [Operations.GetOperation][google.longrunning.Operations.GetOperation] or"]
        #[doc = " other methods to check whether the cancellation succeeded or whether the"]
        #[doc = " operation completed despite cancellation. On successful cancellation,"]
        #[doc = " the operation is not deleted; instead, it becomes an operation with"]
        #[doc = " an [Operation.error][google.longrunning.Operation.error] value with a [google.rpc.Status.code][google.rpc.Status.code] of 1,"]
        #[doc = " corresponding to `Code.CANCELLED`."]
        pub async fn cancel_operation(
            &mut self,
            request: impl tonic::IntoRequest<super::CancelOperationRequest>,
        ) -> Result<tonic::Response<()>, tonic::Status> {
            self.inner.ready().await.map_err(|e| {
                tonic::Status::new(
                    tonic::Code::Unknown,
                    format!("Service was not ready: {}", e.into()),
                )
            })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/google.longrunning.Operations/CancelOperation",
            );
            self.inner.unary(request.into_request(), path, codec).await
        }
        #[doc = " Waits for the specified long-running operation until it is done or reaches"]
        #[doc = " at most a specified timeout, returning the latest state.  If the operation"]
        #[doc = " is already done, the latest state is immediately returned.  If the timeout"]
        #[doc = " specified is greater than the default HTTP/RPC timeout, the HTTP/RPC"]
        #[doc = " timeout is used.  If the server does not support this method, it returns"]
        #[doc = " `google.rpc.Code.UNIMPLEMENTED`."]
        #[doc = " Note that this method is on a best-effort basis.  It may return the latest"]
        #[doc = " state before the specified timeout (including immediately), meaning even an"]
        #[doc = " immediate response is no guarantee that the operation is done."]
        pub async fn wait_operation(
            &mut self,
            request: impl tonic::IntoRequest<super::WaitOperationRequest>,
        ) -> Result<tonic::Response<super::Operation>, tonic::Status> {
            self.inner.ready().await.map_err(|e| {
                tonic::Status::new(
                    tonic::Code::Unknown,
                    format!("Service was not ready: {}", e.into()),
                )
            })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/google.longrunning.Operations/WaitOperation",
            );
            self.inner.unary(request.into_request(), path, codec).await
        }
    }
    impl<T: Clone> Clone for OperationsClient<T> {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
            }
        }
    }
    impl<T> std::fmt::Debug for OperationsClient<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "OperationsClient {{ ... }}")
        }
    }
}
#[doc = r" Generated server implementations."]
pub mod operations_server {
    #![allow(unused_variables, dead_code, missing_docs)]
    use tonic::codegen::*;
    #[doc = "Generated trait containing gRPC methods that should be implemented for use with OperationsServer."]
    #[async_trait]
    pub trait Operations: Send + Sync + 'static {
        #[doc = " Lists operations that match the specified filter in the request. If the"]
        #[doc = " server doesn't support this method, it returns `UNIMPLEMENTED`."]
        #[doc = ""]
        #[doc = " NOTE: the `name` binding allows API services to override the binding"]
        #[doc = " to use different resource name schemes, such as `users/*/operations`. To"]
        #[doc = " override the binding, API services can add a binding such as"]
        #[doc = " `\"/v1/{name=users/*}/operations\"` to their service configuration."]
        #[doc = " For backwards compatibility, the default name includes the operations"]
        #[doc = " collection id, however overriding users must ensure the name binding"]
        #[doc = " is the parent resource, without the operations collection id."]
        async fn list_operations(
            &self,
            request: tonic::Request<super::ListOperationsRequest>,
        ) -> Result<tonic::Response<super::ListOperationsResponse>, tonic::Status>;
        #[doc = " Gets the latest state of a long-running operation.  Clients can use this"]
        #[doc = " method to poll the operation result at intervals as recommended by the API"]
        #[doc = " service."]
        async fn get_operation(
            &self,
            request: tonic::Request<super::GetOperationRequest>,
        ) -> Result<tonic::Response<super::Operation>, tonic::Status>;
        #[doc = " Deletes a long-running operation. This method indicates that the client is"]
        #[doc = " no longer interested in the operation result. It does not cancel the"]
        #[doc = " operation. If the server doesn't support this method, it returns"]
        #[doc = " `google.rpc.Code.UNIMPLEMENTED`."]
        async fn delete_operation(
            &self,
            request: tonic::Request<super::DeleteOperationRequest>,
        ) -> Result<tonic::Response<()>, tonic::Status>;
        #[doc = " Starts asynchronous cancellation on a long-running operation.  The server"]
        #[doc = " makes a best effort to cancel the operation, but success is not"]
        #[doc = " guaranteed.  If the server doesn't support this method, it returns"]
        #[doc = " `google.rpc.Code.UNIMPLEMENTED`.  Clients can use"]
        #[doc = " [Operations.GetOperation][google.longrunning.Operations.GetOperation] or"]
        #[doc = " other methods to check whether the cancellation succeeded or whether the"]
        #[doc = " operation completed despite cancellation. On successful cancellation,"]
        #[doc = " the operation is not deleted; instead, it becomes an operation with"]
        #[doc = " an [Operation.error][google.longrunning.Operation.error] value with a [google.rpc.Status.code][google.rpc.Status.code] of 1,"]
        #[doc = " corresponding to `Code.CANCELLED`."]
        async fn cancel_operation(
            &self,
            request: tonic::Request<super::CancelOperationRequest>,
        ) -> Result<tonic::Response<()>, tonic::Status>;
        #[doc = " Waits for the specified long-running operation until it is done or reaches"]
        #[doc = " at most a specified timeout, returning the latest state.  If the operation"]
        #[doc = " is already done, the latest state is immediately returned.  If the timeout"]
        #[doc = " specified is greater than the default HTTP/RPC timeout, the HTTP/RPC"]
        #[doc = " timeout is used.  If the server does not support this method, it returns"]
        #[doc = " `google.rpc.Code.UNIMPLEMENTED`."]
        #[doc = " Note that this method is on a best-effort basis.  It may return the latest"]
        #[doc = " state before the specified timeout (including immediately), meaning even an"]
        #[doc = " immediate response is no guarantee that the operation is done."]
        async fn wait_operation(
            &self,
            request: tonic::Request<super::WaitOperationRequest>,
        ) -> Result<tonic::Response<super::Operation>, tonic::Status>;
    }
    #[doc = " Manages long-running operations with an API service."]
    #[doc = ""]
    #[doc = " When an API method normally takes long time to complete, it can be designed"]
    #[doc = " to return [Operation][google.longrunning.Operation] to the client, and the client can use this"]
    #[doc = " interface to receive the real response asynchronously by polling the"]
    #[doc = " operation resource, or pass the operation resource to another API (such as"]
    #[doc = " Google Cloud Pub/Sub API) to receive the response.  Any API service that"]
    #[doc = " returns long-running operations should implement the `Operations` interface"]
    #[doc = " so developers can have a consistent client experience."]
    #[derive(Debug)]
    pub struct OperationsServer<T: Operations> {
        inner: _Inner<T>,
    }
    struct _Inner<T>(Arc<T>, Option<tonic::Interceptor>);
    impl<T: Operations> OperationsServer<T> {
        pub fn new(inner: T) -> Self {
            let inner = Arc::new(inner);
            let inner = _Inner(inner, None);
            Self { inner }
        }
        pub fn with_interceptor(inner: T, interceptor: impl Into<tonic::Interceptor>) -> Self {
            let inner = Arc::new(inner);
            let inner = _Inner(inner, Some(interceptor.into()));
            Self { inner }
        }
    }
    impl<T, B> Service<http::Request<B>> for OperationsServer<T>
    where
        T: Operations,
        B: HttpBody + Send + Sync + 'static,
        B::Error: Into<StdError> + Send + 'static,
    {
        type Response = http::Response<tonic::body::BoxBody>;
        type Error = Never;
        type Future = BoxFuture<Self::Response, Self::Error>;
        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: http::Request<B>) -> Self::Future {
            let inner = self.inner.clone();
            match req.uri().path() {
                "/google.longrunning.Operations/ListOperations" => {
                    #[allow(non_camel_case_types)]
                    struct ListOperationsSvc<T: Operations>(pub Arc<T>);
                    impl<T: Operations> tonic::server::UnaryService<super::ListOperationsRequest>
                        for ListOperationsSvc<T>
                    {
                        type Response = super::ListOperationsResponse;
                        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::ListOperationsRequest>,
                        ) -> Self::Future {
                            let inner = self.0.clone();
                            let fut = async move { (*inner).list_operations(request).await };
                            Box::pin(fut)
                        }
                    }
                    let inner = self.inner.clone();
                    let fut = async move {
                        let interceptor = inner.1.clone();
                        let inner = inner.0;
                        let method = ListOperationsSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = if let Some(interceptor) = interceptor {
                            tonic::server::Grpc::with_interceptor(codec, interceptor)
                        } else {
                            tonic::server::Grpc::new(codec)
                        };
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/google.longrunning.Operations/GetOperation" => {
                    #[allow(non_camel_case_types)]
                    struct GetOperationSvc<T: Operations>(pub Arc<T>);
                    impl<T: Operations> tonic::server::UnaryService<super::GetOperationRequest> for GetOperationSvc<T> {
                        type Response = super::Operation;
                        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::GetOperationRequest>,
                        ) -> Self::Future {
                            let inner = self.0.clone();
                            let fut = async move { (*inner).get_operation(request).await };
                            Box::pin(fut)
                        }
                    }
                    let inner = self.inner.clone();
                    let fut = async move {
                        let interceptor = inner.1.clone();
                        let inner = inner.0;
                        let method = GetOperationSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = if let Some(interceptor) = interceptor {
                            tonic::server::Grpc::with_interceptor(codec, interceptor)
                        } else {
                            tonic::server::Grpc::new(codec)
                        };
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/google.longrunning.Operations/DeleteOperation" => {
                    #[allow(non_camel_case_types)]
                    struct DeleteOperationSvc<T: Operations>(pub Arc<T>);
                    impl<T: Operations> tonic::server::UnaryService<super::DeleteOperationRequest>
                        for DeleteOperationSvc<T>
                    {
                        type Response = ();
                        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::DeleteOperationRequest>,
                        ) -> Self::Future {
                            let inner = self.0.clone();
                            let fut = async move { (*inner).delete_operation(request).await };
                            Box::pin(fut)
                        }
                    }
                    let inner = self.inner.clone();
                    let fut = async move {
                        let interceptor = inner.1.clone();
                        let inner = inner.0;
                        let method = DeleteOperationSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = if let Some(interceptor) = interceptor {
                            tonic::server::Grpc::with_interceptor(codec, interceptor)
                        } else {
                            tonic::server::Grpc::new(codec)
                        };
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/google.longrunning.Operations/CancelOperation" => {
                    #[allow(non_camel_case_types)]
                    struct CancelOperationSvc<T: Operations>(pub Arc<T>);
                    impl<T: Operations> tonic::server::UnaryService<super::CancelOperationRequest>
                        for CancelOperationSvc<T>
                    {
                        type Response = ();
                        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::CancelOperationRequest>,
                        ) -> Self::Future {
                            let inner = self.0.clone();
                            let fut = async move { (*inner).cancel_operation(request).await };
                            Box::pin(fut)
                        }
                    }
                    let inner = self.inner.clone();
                    let fut = async move {
                        let interceptor = inner.1.clone();
                        let inner = inner.0;
                        let method = CancelOperationSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = if let Some(interceptor) = interceptor {
                            tonic::server::Grpc::with_interceptor(codec, interceptor)
                        } else {
                            tonic::server::Grpc::new(codec)
                        };
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/google.longrunning.Operations/WaitOperation" => {
                    #[allow(non_camel_case_types)]
                    struct WaitOperationSvc<T: Operations>(pub Arc<T>);
                    impl<T: Operations> tonic::server::UnaryService<super::WaitOperationRequest>
                        for WaitOperationSvc<T>
                    {
                        type Response = super::Operation;
                        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::WaitOperationRequest>,
                        ) -> Self::Future {
                            let inner = self.0.clone();
                            let fut = async move { (*inner).wait_operation(request).await };
                            Box::pin(fut)
                        }
                    }
                    let inner = self.inner.clone();
                    let fut = async move {
                        let interceptor = inner.1.clone();
                        let inner = inner.0;
                        let method = WaitOperationSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = if let Some(interceptor) = interceptor {
                            tonic::server::Grpc::with_interceptor(codec, interceptor)
                        } else {
                            tonic::server::Grpc::new(codec)
                        };
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                _ => Box::pin(async move {
                    Ok(http::Response::builder()
                        .status(200)
                        .header("grpc-status", "12")
                        .body(tonic::body::BoxBody::empty())
                        .unwrap())
                }),
            }
        }
    }
    impl<T: Operations> Clone for OperationsServer<T> {
        fn clone(&self) -> Self {
            let inner = self.inner.clone();
            Self { inner }
        }
    }
    impl<T: Operations> Clone for _Inner<T> {
        fn clone(&self) -> Self {
            Self(self.0.clone(), self.1.clone())
        }
    }
    impl<T: std::fmt::Debug> std::fmt::Debug for _Inner<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:?}", self.0)
        }
    }
    impl<T: Operations> tonic::transport::NamedService for OperationsServer<T> {
        const NAME: &'static str = "google.longrunning.Operations";
    }
}
