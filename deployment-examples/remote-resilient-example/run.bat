@echo off
echo Running NativeLink resilient client with remote execution...
resilient-client.exe --config=client-config.json --target=//myapp:hello_world
pause
