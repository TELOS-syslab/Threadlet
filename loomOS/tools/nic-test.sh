echo -n "hello1" | nc -u -w1 127.0.0.1 11181
sleep 2
echo -n "hello1" | nc -u -w1 127.0.0.1 11181
sleep 2
echo -n "hello2" | nc -u -w1 127.0.0.1 11182
sleep 2
echo -n "hello3" | nc -u -w1 127.0.0.1 11183


# echo -n "hello1" | nc -u -w1 172.16.0.2 11181
# echo -n "hello2" | nc -u -w1 172.16.0.2 11182
# echo -n "hello3" | nc -u -w1 172.16.0.2 11183


# echo -n "hello1" | nc -w1 172.16.0.2 11181
