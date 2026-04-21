# WorkflowAction

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**TaskId** | **string** | The task id to run. | 
**Type** | **string** |  | 
**ProjectId** | **string** | The project id to deploy. | 
**SyncId** | **string** | The sync id to apply. | 

## Methods

### NewWorkflowAction

`func NewWorkflowAction(taskId string, type_ string, projectId string, syncId string, ) *WorkflowAction`

NewWorkflowAction instantiates a new WorkflowAction object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewWorkflowActionWithDefaults

`func NewWorkflowActionWithDefaults() *WorkflowAction`

NewWorkflowActionWithDefaults instantiates a new WorkflowAction object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetTaskId

`func (o *WorkflowAction) GetTaskId() string`

GetTaskId returns the TaskId field if non-nil, zero value otherwise.

### GetTaskIdOk

`func (o *WorkflowAction) GetTaskIdOk() (*string, bool)`

GetTaskIdOk returns a tuple with the TaskId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTaskId

`func (o *WorkflowAction) SetTaskId(v string)`

SetTaskId sets TaskId field to given value.


### GetType

`func (o *WorkflowAction) GetType() string`

GetType returns the Type field if non-nil, zero value otherwise.

### GetTypeOk

`func (o *WorkflowAction) GetTypeOk() (*string, bool)`

GetTypeOk returns a tuple with the Type field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetType

`func (o *WorkflowAction) SetType(v string)`

SetType sets Type field to given value.


### GetProjectId

`func (o *WorkflowAction) GetProjectId() string`

GetProjectId returns the ProjectId field if non-nil, zero value otherwise.

### GetProjectIdOk

`func (o *WorkflowAction) GetProjectIdOk() (*string, bool)`

GetProjectIdOk returns a tuple with the ProjectId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProjectId

`func (o *WorkflowAction) SetProjectId(v string)`

SetProjectId sets ProjectId field to given value.


### GetSyncId

`func (o *WorkflowAction) GetSyncId() string`

GetSyncId returns the SyncId field if non-nil, zero value otherwise.

### GetSyncIdOk

`func (o *WorkflowAction) GetSyncIdOk() (*string, bool)`

GetSyncIdOk returns a tuple with the SyncId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSyncId

`func (o *WorkflowAction) SetSyncId(v string)`

SetSyncId sets SyncId field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


