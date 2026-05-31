# NodeAffinityOneOf

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Kind** | **string** |  | 
**NodeIds** | **[]string** | The set of &#x60;node_id&#x60;s entitled to host this secret. | 

## Methods

### NewNodeAffinityOneOf

`func NewNodeAffinityOneOf(kind string, nodeIds []string, ) *NodeAffinityOneOf`

NewNodeAffinityOneOf instantiates a new NodeAffinityOneOf object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNodeAffinityOneOfWithDefaults

`func NewNodeAffinityOneOfWithDefaults() *NodeAffinityOneOf`

NewNodeAffinityOneOfWithDefaults instantiates a new NodeAffinityOneOf object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetKind

`func (o *NodeAffinityOneOf) GetKind() string`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *NodeAffinityOneOf) GetKindOk() (*string, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *NodeAffinityOneOf) SetKind(v string)`

SetKind sets Kind field to given value.


### GetNodeIds

`func (o *NodeAffinityOneOf) GetNodeIds() []string`

GetNodeIds returns the NodeIds field if non-nil, zero value otherwise.

### GetNodeIdsOk

`func (o *NodeAffinityOneOf) GetNodeIdsOk() (*[]string, bool)`

GetNodeIdsOk returns a tuple with the NodeIds field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeIds

`func (o *NodeAffinityOneOf) SetNodeIds(v []string)`

SetNodeIds sets NodeIds field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


