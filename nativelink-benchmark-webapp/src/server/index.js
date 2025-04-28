const express = require('express');
const cors = require('cors');
const morgan = require('morgan');
const path = require('path');
const mongoose = require('mongoose');
require('dotenv').config();

const app = express();

// Middleware
app.use(cors());
app.use(morgan('dev'));
app.use(express.json());

// MongoDB Schema
const benchmarkSchema = new mongoose.Schema({
  commit: String,
  timestamp: Date,
  buildTime: Number,
  buildType: String, // 'remoteCacheOnly' or 'remoteCacheAndExecution'
  metrics: {
    cpuUsage: Number,
    memoryUsage: Number,
    networkLatency: Number
  }
});

const Benchmark = mongoose.model('Benchmark', benchmarkSchema);

// API Routes
app.get('/api/benchmarks/latest', async (req, res) => {
  try {
    const remoteCacheOnly = await Benchmark
      .find({ buildType: 'remoteCacheOnly' })
      .sort({ timestamp: -1 })
      .limit(10);

    const remoteCacheAndExecution = await Benchmark
      .find({ buildType: 'remoteCacheAndExecution' })
      .sort({ timestamp: -1 })
      .limit(10);

    res.json({
      remoteCacheOnly,
      remoteCacheAndExecution
    });
  } catch (error) {
    res.status(500).json({ error: error.message });
  }
});

// Serve static files in production
if (process.env.NODE_ENV === 'production') {
  app.use(express.static(path.join(__dirname, '../../client/build')));
  app.get('*', (req, res) => {
    res.sendFile(path.join(__dirname, '../../client/build/index.html'));
  });
}

// Connect to MongoDB and start server
const PORT = process.env.PORT || 5000;
mongoose
  .connect(process.env.MONGODB_URI || 'mongodb://localhost/nativelink-benchmarks')
  .then(() => {
    app.listen(PORT, () => {
      console.log(`Server running on port ${PORT}`);
    });
  })
  .catch((error) => {
    console.error('Database connection error:', error);
  });