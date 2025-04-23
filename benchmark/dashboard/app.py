from flask import Flask, render_template, jsonify
import pymongo
import os
import json
from datetime import datetime, timedelta

app = Flask(__name__)

# Connect to MongoDB
client = pymongo.MongoClient(os.environ.get("BENCHMARK_DB_URI"))
db = client.nativelink_benchmarks
collection = db.benchmark_results

@app.route('/')
def index():
    return render_template('index.html')

@app.route('/api/results')
def get_results():
    # Get the last 30 days of results
    thirty_days_ago = datetime.now() - timedelta(days=30)
    
    results = list(collection.find({
        "metadata.timestamp": {"$gte": thirty_days_ago.isoformat()}
    }))
    
    # Convert ObjectId to string for JSON serialization
    for result in results:
        result["_id"] = str(result["_id"])
    
    return jsonify(results)

@app.route('/api/projects')
def get_projects():
    projects = collection.distinct("metadata.project")
    return jsonify(projects)

if __name__ == '__main__':
    app.run(debug=True, host='0.0.0.0')