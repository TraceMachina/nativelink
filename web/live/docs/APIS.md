# Datahouse
Datahouse saves every activities from nativelink instances for metrics and provide APIs for querying.

## APIs
- `GET /detail/:id` - Get the detail of a specific action by its ID. Datahouse will collect when: action is first created and then completed.

# Live Activity Monitor
This service should collect every components from nativelink instances and then expose them via WebSocket.
- WebSocket endpoint: `/ws?version=<NATIVELINK_LIVE_VERISON>`
- For WebSocket formats, see [WEBSOCKETS.md](WEBSOCKETS.md)

## APIs
- `GET /activities/overall` - Get the overall statistics of activities.
- `GET /status/:id` - Get the status of a specific action by its ID. The service will cache the status for a while even if the action is completed.
