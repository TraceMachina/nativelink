import React from 'react';
import { Bar } from 'react-chartjs-2';
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  BarElement,
  Title,
  Tooltip,
  Legend,
} from 'chart.js';

ChartJS.register(
  CategoryScale,
  LinearScale,
  BarElement,
  Title,
  Tooltip,
  Legend
);

export default function BenchmarkDetails({ benchmark }) {
  if (!benchmark) return null;

  const metrics = benchmark.metrics || {};
  const networkStats = benchmark.network_stats || {};
  const systemStats = benchmark.system_stats || {};
  
  // Extraire les données spécifiques pour les deux cas de test
  const remoteCacheOnly = benchmark.remote_cache_only || {};
  const remoteCacheExecution = benchmark.remote_cache_execution || {};

  const buildTimeData = {
    labels: ['Remote Cache Only', 'Remote Cache + Execution'],
    datasets: [
      {
        label: 'Temps de construction (secondes)',
        data: [
          remoteCacheOnly.build_time || metrics.remote_cache_only_time || 0,
          remoteCacheExecution.build_time || metrics.remote_cache_execution_time || 0
        ],
        backgroundColor: ['rgba(54, 162, 235, 0.5)', 'rgba(153, 102, 255, 0.5)'],
      },
    ],
  };

  const networkData = {
    labels: ['Remote Cache Only', 'Remote Cache + Execution'],
    datasets: [
      {
        label: 'Octets envoyés',
        data: [
          remoteCacheOnly.bytes_sent || networkStats.remote_cache_only_sent || 0,
          remoteCacheExecution.bytes_sent || networkStats.remote_cache_execution_sent || 0
        ],
        backgroundColor: 'rgba(255, 99, 132, 0.5)',
      },
      {
        label: 'Octets reçus',
        data: [
          remoteCacheOnly.bytes_recv || networkStats.remote_cache_only_recv || 0,
          remoteCacheExecution.bytes_recv || networkStats.remote_cache_execution_recv || 0
        ],
        backgroundColor: 'rgba(75, 192, 192, 0.5)',
      },
    ],
  };
  
  // Données pour les statistiques système
  const systemData = {
    labels: ['Remote Cache Only', 'Remote Cache + Execution'],
    datasets: [
      {
        label: 'CPU (%)',
        data: [
          remoteCacheOnly.cpu_percent || systemStats.remote_cache_only_cpu || 0,
          remoteCacheExecution.cpu_percent || systemStats.remote_cache_execution_cpu || 0
        ],
        backgroundColor: 'rgba(255, 206, 86, 0.5)',
      },
      {
        label: 'Mémoire (MB)',
        data: [
          remoteCacheOnly.memory_mb || systemStats.remote_cache_only_memory || 0,
          remoteCacheExecution.memory_mb || systemStats.remote_cache_execution_memory || 0
        ],
        backgroundColor: 'rgba(153, 102, 255, 0.5)',
      },
    ],
  };

  return (
    <div className="benchmark-details">
      <h2>Détails du benchmark</h2>
      <p className="benchmark-description">
        Comparaison des performances entre <strong>Remote Cache Only</strong> et <strong>Remote Cache + Execution</strong>
      </p>
      
      <div className="chart">
        <h3>Temps de construction</h3>
        <Bar data={buildTimeData} />
      </div>
      
      <div className="chart">
        <h3>Statistiques réseau</h3>
        <Bar data={networkData} />
      </div>
      
      <div className="chart">
        <h3>Utilisation des ressources système</h3>
        <Bar data={systemData} />
      </div>
      
      <div className="benchmark-summary">
        <h3>Résumé</h3>
        <div className="summary-grid">
          <div className="summary-card">
            <h4>Remote Cache Only</h4>
            <p>Temps de construction: {remoteCacheOnly.build_time || metrics.remote_cache_only_time || 0} secondes</p>
            <p>Données envoyées: {(remoteCacheOnly.bytes_sent || networkStats.remote_cache_only_sent || 0) / (1024 * 1024)} MB</p>
            <p>Données reçues: {(remoteCacheOnly.bytes_recv || networkStats.remote_cache_only_recv || 0) / (1024 * 1024)} MB</p>
          </div>
          <div className="summary-card">
            <h4>Remote Cache + Execution</h4>
            <p>Temps de construction: {remoteCacheExecution.build_time || metrics.remote_cache_execution_time || 0} secondes</p>
            <p>Données envoyées: {(remoteCacheExecution.bytes_sent || networkStats.remote_cache_execution_sent || 0) / (1024 * 1024)} MB</p>
            <p>Données reçues: {(remoteCacheExecution.bytes_recv || networkStats.remote_cache_execution_recv || 0) / (1024 * 1024)} MB</p>
          </div>
        </div>
      </div>
      
      <style jsx>{`
        .benchmark-details {
          margin-top: 2rem;
          padding: 1rem;
          background-color: #f9f9f9;
          border-radius: 8px;
          box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
        }
        .benchmark-description {
          text-align: center;
          margin-bottom: 2rem;
          color: #555;
        }
        .chart {
          margin: 2rem 0;
          padding: 1rem;
          background-color: white;
          border-radius: 8px;
          box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
        }
        .benchmark-summary {
          margin-top: 2rem;
        }
        .summary-grid {
          display: grid;
          grid-template-columns: 1fr 1fr;
          gap: 1rem;
        }
        .summary-card {
          padding: 1rem;
          background-color: white;
          border-radius: 8px;
          box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
        }
        h4 {
          margin-top: 0;
          color: #333;
          border-bottom: 1px solid #eee;
          padding-bottom: 0.5rem;
        }
      `}</style>
    </div>
  );
}