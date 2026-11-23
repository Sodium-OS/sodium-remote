@echo off
cd /d %~dp0
if exist debug.flag set SODIUM_REMOTE_HOST_DEBUG=1
sodium-remote-host.exe --data %~dp0data --pair --no-mdns --name "Sodium Remote test VM" > host.out 2> host.log
