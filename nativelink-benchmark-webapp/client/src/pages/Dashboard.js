import React, { useState, useEffect } from 'react';
import { Box, Grid, Paper, Typography } from '@mui/material';
import { Line } from 'react-chartjs-2';
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend
} from 'chart.js';

ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend
);

const Dashboard = () => {
  const [buildData, setBuildData] = useState({
    remoteCacheOnly: [],
    remoteCacheAndExecution: []
  });

  useEffect(() => {
    // TODO: Fetch real data from API
    const fetchData = async () => {
      try {
        const response = await fetch('/api/benchmarks/latest');
        const data = await response.json();
        setBuildData(data);
      } catch (error) {
        console.error('Error fetching benchmark data:', error);
      }
    };

    fetchData();
  }, []);

  const chartOptions = {
    responsive: true,
    plugins: {
      legend: {
        position: 'top',
      },
      title: {
        display: true,
        text: 'Build Performance Trends',
      },
    },
  };

  return (
    <Box sx={{ flexGrow: 1 }}>
      <Typography variant="h4" gutterBottom>
        NativeLink Build Performance Dashboard
      </Typography>
      
      <Grid container spacing={3}>
        <Grid item xs={12} md={6}>
          <Paper sx={{ p: 2 }}>
            <Typography variant="h6" gutterBottom>
              Remote Cache Only
            </Typography>
            <Line
              options={chartOptions}
              data={{
                labels: buildData.remoteCacheOnly.map(d => d.commit.substring(0, 7)),
                datasets: [{
                  label: 'Build Time (s)',
                  data: buildData.remoteCacheOnly.map(d => d.buildTime),
                  borderColor: 'rgb(75, 192, 192)',
                  tension: 0.1
                }]
              }}
            />
          </Paper>
        </Grid>

        <Grid item xs={12} md={6}>
          <Paper sx={{ p: 2 }}>
            <Typography variant="h6" gutterBottom>
              Remote Cache + Execution
            </Typography>
            <Line
              options={chartOptions}
              data={{
                labels: buildData.remoteCacheAndExecution.map(d => d.commit.substring(0, 7)),
                datasets: [{
                  label: 'Build Time (s)',
                  data: buildData.remoteCacheAndExecution.map(d => d.buildTime),
                  borderColor: 'rgb(153, 102, 255)',
                  tension: 0.1
                }]
              }}
            />
          </Paper>
        </Grid>
      </Grid>
    </Box>
  );
};

export default Dashboard;