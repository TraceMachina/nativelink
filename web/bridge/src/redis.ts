import { createClient } from 'redis';

export async function initializeRedisClients() {
  try {
    const redisClient = createClient({
      socket: {
        host: "172.17.0.2",
        port: 6379
      }
    });
    const commandClient = redisClient.duplicate();

    redisClient.on('error', (err) => {
      console.error('Redis Client Error:', err);
      throw new Error('Failed to connect to Redis.');
    });

    await redisClient.connect();
    await commandClient.connect();

    console.log('\nRedis clients successfully connected.\n');

    return { redisClient, commandClient };
  } catch (error) {
    console.error('Error during Redis client initialization:', error);
    throw new Error('Unable to initialize Redis clients. Please check your connection.');
  }
}
