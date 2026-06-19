https://redis.io/docs/latest/develop/pubsub/keyspace-notifications/#events-generated-by-different-commands
https://redis.io/docs/latest/develop/reference/eviction/

config set maxmemory 10mb
config set maxmemory-policy allkeys-lru
config set notify-keyspace-events Ke

redis-cli --csv psubscribe '__key*__:*'

"pmessage","__key*__:*","__keyspace@0__:name78483","evicted"
