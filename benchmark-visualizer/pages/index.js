import { useState, useEffect } from 'react';
import { Line } from 'react-chartjs-2';
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend,
} from 'chart.js';
import BenchmarkDetails from '../components/BenchmarkDetails';

ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend
);

export default function Home() {
  const [benchmarkData, setBenchmarkData] = useState([]);
  const [selectedCommit, setSelectedCommit] = useState(null);

  useEffect(() => {
    fetch('/api/benchmarks')
      .then(res => res.json())
      .then(data => setBenchmarkData(data));
  }, []);

  const chartData = {
    labels: Array.isArray(benchmarkData) ? benchmarkData.map(item => item.commit.slice(0, 7)) : [],
    datasets: [
      {
        label: 'Build Time (seconds)',
        data: Array.isArray(benchmarkData) ? benchmarkData.map(item => item.build_remote_execution_time_sec) : [],
        borderColor: 'rgb(75, 192, 192)',
        backgroundColor: 'rgba(75, 192, 192, 0.5)',
      },
    ],
  };

  const options = {
    responsive: true,
    onClick: (e, elements) => {
      if (elements.length > 0) {
        const index = elements[0].index;
        setSelectedCommit(benchmarkData[index]);
      }
    },
  };

  return (
    <div className="container">
      <h1>NativeLink Benchmark Results</h1>
      <div className="chart-container">
        <Line data={chartData} options={options} />
      </div>
      
      {selectedCommit && (
        <div className="commit-details">
          <h2>Commit: {selectedCommit.commit}</h2>
          <p>Date: {new Date(selectedCommit.timestamp || 0).toLocaleString()}</p>
          <BenchmarkDetails benchmark={selectedCommit} />
        </div>
      )}

      <style jsx>{`
        .container {
          max-width: 1200px;
          margin: 0 auto;
          padding: 20px;
        }
        .chart-container {
          margin: 40px 0;
        }
        .commit-details {
          margin-top: 40px;
          padding: 20px;
          background: #f5f5f5;
          border-radius: 4px;
        }
      `}</style>
    </div>
  );
}