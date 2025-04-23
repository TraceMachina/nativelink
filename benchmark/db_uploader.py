#!/usr/bin/env python3

import argparse
import json
import os
from pathlib import Path
import pymongo
from datetime import datetime

def upload_results(results_dir, db_uri):
    """Upload benchmark results to MongoDB"""
    # Connect to MongoDB
    client = pymongo.MongoClient(db_uri)
    db = client.nativelink_benchmarks
    collection = db.benchmark_results
    
    # Load all benchmark results
    results_dir = Path(results_dir)
    count = 0
    
    for result_file in results_dir.glob("*.json"):
        with open(result_file, 'r') as f:
            data = json.load(f)
            
            # Add upload timestamp
            data["uploaded_at"] = datetime.now().isoformat()
            
            # Insert into database
            collection.insert_one(data)
            count += 1
    
    print(f"Uploaded {count} benchmark results to database")

def main():
    parser = argparse.ArgumentParser(description='Upload NativeLink benchmark results to database')
    parser.add_argument('--results-dir', required=True, help='Directory with benchmark results')
    parser.add_argument('--db-uri', required=True, help='MongoDB connection URI')
    
    args = parser.parse_args()
    upload_results(args.results_dir, args.db_uri)

if __name__ == "__main__":
    main()